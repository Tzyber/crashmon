//! Aggregator-Loop-Tests auf Daemon-Ebene (Phase 2.5, Review-Regressionen).
//!
//! Treibt `daemon::aggregator_loop` direkt (kein Host-Journal noetig —
//! laeuft in normalem `cargo test`):
//! - Review 2.5 CRITICAL: push liefert fertigen Report -> MUSS sofort
//!   geschrieben werden (wurde verworfen, Timer invertiert)
//! - Timer-Pfad: offene Gruppe -> Report nach WINDOW+SLACK
//! - Shutdown-Pfad: Sender-Drop -> Drain + Flush (kein Event-Verlust)

use crash_daemon::aggregate::channel;
use crash_daemon::daemon::aggregator_loop;
use crash_daemon::event::{CrashEvent, EventKind};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::Duration;

fn temp_dir(name: &str) -> std::path::PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("crashmon-aggloop-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).expect("temp dir");
    p
}

fn coredump(pid: u32, ts: u64) -> CrashEvent {
    CrashEvent {
        ts,
        kind: EventKind::Coredump {
            pid,
            exe: None,
            comm: "app".into(),
            signal: None,
            uid: None,
            unit: None,
            coredump_file: None,
        },
    }
}

fn gpu_reset(ts: u64) -> CrashEvent {
    CrashEvent {
        ts,
        kind: EventKind::GpuReset {
            vendor: "amdgpu".into(),
            detail: "amdgpu: GPU reset begin!".into(),
        },
    }
}

fn report_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    std::fs::read_dir(dir)
        .map(|it| {
            it.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .unwrap()
                        .to_str()
                        .unwrap()
                        .starts_with("crash-")
                })
                .collect()
        })
        .unwrap_or_default()
}

async fn wait_for(dir: &std::path::Path, count: usize) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline && report_files(dir).len() < count {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn push_closing_group_writes_report_immediately() {
    // Regression Review 2.5 CRITICAL: Event B (fremde Klasse) schliesst die
    // offene Gruppe — push liefert Some(report), das MUSS sofort auf die
    // Platte (wurde frueher verworfen).
    let dir = temp_dir("close");
    let (sender, rx) = channel();
    let set = tokio::task::LocalSet::new();
    let a = set.spawn_local(aggregator_loop(
        rx,
        Arc::new(AtomicU64::new(0)),
        dir.clone(),
    ));

    // Coredump(pid A) eroeffnet Gruppe; GpuReset (fremd) schliesst sie.
    // WICHTIG: Warteschleife im run_until laufen lassen — nur dort tickt
    // das LocalSet die spawn_local-Task.
    sender.try_send(coredump(1, 1_000_000));
    sender.try_send(gpu_reset(1_000_100));
    set.run_until(wait_for(&dir, 1)).await;

    assert_eq!(
        report_files(&dir).len(),
        1,
        "push-Close muss sofort einen Report schreiben (kein Timer/Shutdown noetig)"
    );

    // Aufraeumen: Sender droppen -> Loop endet ueber rx.recv()==None
    drop(sender);
    set.run_until(async {
        let _ = a.await;
    })
    .await;
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test(flavor = "current_thread")]
async fn timer_flushes_open_group() {
    // Timer-Pfad: einzelnes Event -> Report nach WINDOW+SLACK (5,1 s)
    let dir = temp_dir("timer");
    let (sender, rx) = channel();
    let set = tokio::task::LocalSet::new();
    let a = set.spawn_local(aggregator_loop(
        rx,
        Arc::new(AtomicU64::new(0)),
        dir.clone(),
    ));

    sender.try_send(coredump(2, 2_000_000));
    set.run_until(wait_for(&dir, 1)).await;

    assert_eq!(report_files(&dir).len(), 1, "Timer-Flush nach ~5,1 s");

    drop(sender);
    set.run_until(async {
        let _ = a.await;
    })
    .await;
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test(flavor = "current_thread")]
async fn shutdown_drop_flushes_remaining() {
    // Shutdown-Pfad (Review 2.5 MAJOR): Sender-Drop -> Drain + Flush —
    // Events im Kanal gehen nicht verloren, offene Gruppe wird geschlossen.
    let dir = temp_dir("shutdown");
    let (sender, rx) = channel();
    let set = tokio::task::LocalSet::new();
    let a = set.spawn_local(aggregator_loop(
        rx,
        Arc::new(AtomicU64::new(0)),
        dir.clone(),
    ));

    // Event senden und SOFORT droppen (kein Timer-Wait)
    sender.try_send(coredump(3, 3_000_000));
    drop(sender);
    set.run_until(async {
        let _ = a.await;
    })
    .await;

    assert_eq!(
        report_files(&dir).len(),
        1,
        "Sender-Drop muss via Drain+Flush den Report schreiben"
    );
    std::fs::remove_dir_all(&dir).ok();
}
