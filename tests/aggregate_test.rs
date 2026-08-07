//! Aggregator-Tests (Phase 2.4, Specs AG-1..3, AG-INV-1/3).
//!
//! Kern ist synchron testbar: push/flush ohne Runtime.

use crash_daemon::aggregate::{Aggregator, CHANNEL_CAPACITY, WINDOW, channel};
use crash_daemon::event::{CrashEvent, EventKind};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

const T0: u64 = 1_000_000_000_000; // willkuerliche µs-Epoch-Basis

/// Frischer Aggregator ohne Verluste.
fn fresh() -> Aggregator {
    Aggregator::new(Arc::new(AtomicU64::new(0)))
}

fn xid(code: u16, pci: Option<&str>, ts: u64) -> CrashEvent {
    CrashEvent {
        ts,
        kind: EventKind::GpuXid {
            code,
            pci: pci.map(str::to_owned),
            pid: None,
            message: format!("NVRM: Xid: {code}"),
        },
    }
}

fn xid_with_pid(code: u16, pid: u32, ts: u64) -> CrashEvent {
    CrashEvent {
        ts,
        kind: EventKind::GpuXid {
            code,
            pci: Some("0000:03:00".into()),
            pid: Some(pid),
            message: format!("NVRM: Xid: {code}, pid='{pid}'"),
        },
    }
}

fn coredump(pid: u32, ts: u64) -> CrashEvent {
    CrashEvent {
        ts,
        kind: EventKind::Coredump {
            pid,
            exe: Some("/usr/bin/app".into()),
            comm: "app".into(),
            signal: Some("SIGSEGV".into()),
            uid: Some(1000),
            unit: None,
            coredump_file: None,
        },
    }
}

fn oom(pid: u32, ts: u64) -> CrashEvent {
    CrashEvent {
        ts,
        kind: EventKind::OomKill {
            pid,
            comm: "app".into(),
        },
    }
}

fn code_of(report: &crash_daemon::output::Report) -> u16 {
    match &report.cause.kind {
        EventKind::GpuXid { code, .. } => *code,
        other => panic!("kein Xid: {other:?}"),
    }
}

// --- AG-1: Xid-Burst T0-Prinzip ------------------------------------------

#[test]
fn xid_burst_t0() {
    // Arrange: 31 -> 43 -> 45, alle innerhalb 5 s
    let mut agg = fresh();
    agg.push(xid(31, Some("0000:03:00"), T0));
    agg.push(xid(43, Some("0000:03:00"), T0 + 1_000_000));
    agg.push(xid(45, Some("0000:03:00"), T0 + 2_000_000));

    // Act
    let report = agg.flush().expect("flush liefert Report");

    // Assert: EIN Report, Ursache = erster Xid (31), Folge-Xids beigeordnet
    assert_eq!(code_of(&report), 31);
    assert_eq!(report.related.len(), 2);
    assert!(matches!(
        report.related[0].kind,
        EventKind::GpuXid { code: 43, .. }
    ));
    assert!(matches!(
        report.related[1].kind,
        EventKind::GpuXid { code: 45, .. }
    ));
}

#[test]
fn xid_burst_with_gap_opens_new_group() {
    // Arrange: 31, dann 79 nach 10 s — Fenster ueberschritten
    let mut agg = fresh();
    agg.push(xid(31, Some("0000:03:00"), T0));

    // Act: naechstes Event schliesst die erste Gruppe
    let report = agg
        .push(xid(79, Some("0000:03:00"), T0 + 10_000_000))
        .expect("Report");

    // Assert: erste Gruppe = nur 31, keine Beigabe
    assert_eq!(code_of(&report), 31);
    assert!(report.related.is_empty());
}

#[test]
fn xid_burst_different_pci_not_grouped() {
    // Arrange: 31 (GPU A), 43 (GPU B) — andere GPU = eigener Report
    let mut agg = fresh();
    agg.push(xid(31, Some("0000:03:00"), T0));

    // Act
    let report = agg
        .push(xid(43, Some("0000:04:00"), T0 + 1_000_000))
        .expect("Report");

    // Assert: 43 ist NICHT beigeordnet
    assert!(report.related.is_empty());
}

// --- AG-2: PID + Zeitfenster ---------------------------------------------

#[test]
fn coredump_and_xid_same_pid_grouped() {
    // Arrange: Coredump(1234) + Xid(pid 1234) innerhalb 5 s
    let mut agg = fresh();
    agg.push(coredump(1234, T0));
    agg.push(xid_with_pid(31, 1234, T0 + 2_000_000));

    // Act
    let report = agg.flush().expect("Report");

    // Assert: ein Report; Ursache = Coredump (frueherer ts)
    assert!(matches!(
        report.cause.kind,
        EventKind::Coredump { pid: 1234, .. }
    ));
    assert_eq!(report.related.len(), 1);
    assert!(matches!(
        report.related[0].kind,
        EventKind::GpuXid { code: 31, .. }
    ));
}

#[test]
fn coredump_and_xid_different_pid_not_grouped() {
    // Arrange: verschiedene PIDs
    let mut agg = fresh();
    agg.push(coredump(1234, T0));

    // Act
    let report = agg
        .push(xid_with_pid(31, 9999, T0 + 1_000_000))
        .expect("Report");

    // Assert: keine Korrelation
    assert!(report.related.is_empty());
}

#[test]
fn coredump_and_oom_same_pid_grouped() {
    // Arrange: OOM-Kill (Ursache) + Coredump (Effekt), gleiche PID
    let mut agg = fresh();
    agg.push(oom(42, T0));
    agg.push(coredump(42, T0 + 1_000_000));

    // Act
    let report = agg.flush().expect("Report");

    // Assert: ein Report, Ursache = OOM (frueher)
    assert!(matches!(
        report.cause.kind,
        EventKind::OomKill { pid: 42, .. }
    ));
    assert_eq!(report.related.len(), 1);
}

// --- Regressionen (Review 2.4, HIGH): 3-Event-Ketten -----------------------

#[test]
fn coredump_then_xid_pid_then_pidless_xid_stays_one_group() {
    // Realer NVIDIA-Burst: Xid 31 traegt die faulting PID, Folge-Xids 43/45
    // meist ohne PID. Alle auf derselben GPU innerhalb 2 s -> EIN Report.
    let mut agg = fresh();
    agg.push(coredump(100, T0));
    agg.push(xid_with_pid(31, 100, T0 + 1_000_000));
    agg.push(xid(43, Some("0000:03:00"), T0 + 2_000_000));
    agg.push(xid(45, Some("0000:03:00"), T0 + 3_000_000));

    // Act
    let report = agg.flush().expect("Report");

    // Assert: alles in einem Report (Ursache = Coredump, fruehester ts)
    assert!(matches!(
        report.cause.kind,
        EventKind::Coredump { pid: 100, .. }
    ));
    assert_eq!(report.related.len(), 3, "alle 3 Xids beigeordnet");
}

#[test]
fn oom_then_coredump_then_xid_stays_one_group() {
    // OomKill(42) -> Coredump(42) -> Xid(pid=42): Mittel-Event verankert die Kette
    let mut agg = fresh();
    agg.push(oom(42, T0));
    agg.push(coredump(42, T0 + 1_000_000));
    agg.push(xid_with_pid(31, 42, T0 + 2_000_000));

    // Act
    let report = agg.flush().expect("Report");

    // Assert: ein Report mit allen dreien
    assert_eq!(report.related.len(), 2);
}

// --- AG-INV-1: Sortierung nach ts -----------------------------------------

#[test]
fn unsorted_input_sorted_by_ts() {
    // Arrange: 43 kommt VOR 31 an (Empfangsreihenfolge != ts)
    let mut agg = fresh();
    agg.push(xid(43, Some("0000:03:00"), T0 + 2_000_000));
    agg.push(xid(31, Some("0000:03:00"), T0));

    // Act
    let report = agg.flush().expect("Report");

    // Assert: Ursache = fruehester ts (31), nicht der zuerst empfangene (43)
    assert_eq!(code_of(&report), 31);
}

// --- AG-INV-3: Report erst nach Fenster-Schluss ----------------------------

#[test]
fn push_closes_window_on_far_event() {
    // Arrange: erstes Event, dann zweites weit ausserhalb des Fensters
    let mut agg = fresh();
    agg.push(xid(31, Some("0000:03:00"), T0));

    // Act: push liefert sofort den Report der geschlossenen Gruppe
    let report = agg
        .push(xid(
            31,
            Some("0000:03:00"),
            T0 + WINDOW.as_micros() as u64 + 1,
        ))
        .expect("Report");

    // Assert: erste Gruppe komplett, kein halber Report
    assert_eq!(code_of(&report), 31);
    assert!(report.related.is_empty());
}

#[test]
fn flush_without_events_is_none() {
    assert!(fresh().flush().is_none());
}

// --- AG-3: Lost-Zaehler im Report -----------------------------------------

#[test]
fn lost_counter_lands_in_report() {
    // Arrange: 1 Event verloren (Zaehler vorher um 1 hochgezogen)
    let (sender, _rx) = channel();
    let lost = sender.lost_counter();
    lost.fetch_add(1, Ordering::Relaxed);
    let mut agg = Aggregator::new(lost);
    agg.push(xid(31, Some("0000:03:00"), T0));

    // Act + Assert
    let report = agg.flush().expect("Report");
    assert_eq!(report.lost_events, 1);
}

// --- B3: GpuReset/OomKill/Wedged-Gruppierung ------------------------------

fn gpu_reset(vendor: &str, ts: u64) -> CrashEvent {
    CrashEvent {
        ts,
        kind: EventKind::GpuReset {
            vendor: vendor.into(),
            detail: format!("{vendor}: GPU reset begin!"),
        },
    }
}

fn wedged(ts: u64) -> CrashEvent {
    CrashEvent {
        ts,
        kind: EventKind::GpuWedged {
            method: Some("rebind".into()),
            device: None,
        },
    }
}

#[test]
fn amdgpu_hang_sequence_is_one_report() {
    // B3-Regression: AMD-Hang schreibt 3-4 matchende Zeilen in ~500 ms
    // (ring timeout, reset begin, reset succeeded) — vorher 4 separate
    // Reports, weil GpuReset↔GpuReset nicht gruppiert wurde.
    let mut agg = fresh();
    agg.push(gpu_reset("amdgpu", T0));
    agg.push(gpu_reset("amdgpu", T0 + 100_000));
    agg.push(gpu_reset("amdgpu", T0 + 400_000));

    let report = agg.flush().expect("Report");
    assert_eq!(report.related.len(), 2, "ein Hang = ein Report");
    assert!(matches!(
        report.cause.kind,
        EventKind::GpuReset { ref vendor, .. } if vendor == "amdgpu"
    ));
}

#[test]
fn different_vendor_resets_not_grouped() {
    // Zwei GPUs (z. B. iGPU + dGPU) reseten unabhaengig -> getrennte Reports
    let mut agg = fresh();
    agg.push(gpu_reset("amdgpu", T0));
    let report = agg.push(gpu_reset("i915", T0 + 1_000_000)).expect("Report");
    assert!(report.related.is_empty());
}

#[test]
fn oom_cascade_is_one_report() {
    // B3-Regression: Kernel killt bei OOM mehrere Prozesse — ein Ereignis
    let mut agg = fresh();
    agg.push(oom(11, T0));
    agg.push(oom(12, T0 + 100_000));
    agg.push(oom(13, T0 + 300_000));

    let report = agg.flush().expect("Report");
    assert_eq!(report.related.len(), 2, "Kaskade = ein Report");
    assert!(matches!(
        report.cause.kind,
        EventKind::OomKill { pid: 11, .. }
    ));
}

#[test]
fn wedged_groups_with_reset() {
    // Wedged-Event + Reset derselben Störung -> ein Report
    let mut agg = fresh();
    agg.push(gpu_reset("amdgpu", T0));
    agg.push(wedged(T0 + 500_000));

    let report = agg.flush().expect("Report");
    assert_eq!(report.related.len(), 1);
    assert!(matches!(
        report.related[0].kind,
        EventKind::GpuWedged { .. }
    ));
}

#[test]
fn channel_full_drops_newest_and_counts() {
    // Echter AG-3-Mechanismus (Review 2.4 MAJOR): Kanal voll -> try_send
    // verwirft das NEUE Event und inkrementiert den Lost-Zaehler.
    // tokio-mpsc try_send/try_recv braucht keine Runtime.
    let (sender, mut rx) = channel();
    for i in 0..CHANNEL_CAPACITY {
        sender.try_send(xid(31, None, T0 + i as u64));
    }

    // Neues Event: Kanal voll -> drop-newest (Event verworfen) + Zaehler.
    // WICHTIG: kein try_recv davor — das wuerde einen Slot freimachen.
    sender.try_send(xid(31, None, T0 + 999_999_999));
    assert_eq!(sender.lost_counter().load(Ordering::Relaxed), 1);

    // Aeltestes Event unversehrt; das verworfenen (juengstes) fehlt.
    assert_eq!(rx.try_recv().map(|e| e.ts), Ok(T0), "aeltestes bleibt");
    let mut max_ts = 0;
    while let Ok(e) = rx.try_recv() {
        max_ts = max_ts.max(e.ts);
    }
    assert_ne!(max_ts, T0 + 999_999_999, "juengstes Event wurde verworfen");
}
