//! Matcher-Fixtures (Phase 2.1, TDD). Quellen: recherche-phase1.md
//! (JSON-Beispiele + reale Kernel-/Treiberzeilen) + NVIDIA-Xid-Doku.

use crash_daemon::event::EventKind;
use crash_daemon::gpu::matcher::{match_message, parse_coredump, xid_severity};

const MESSAGE_ID_COREDUMP: &str = "fc2e22bc6ee647b6b90729ab34a250b1";

// --- OOM-Killer -----------------------------------------------------------

#[test]
fn oom_killed_process_pattern() {
    let msg = "Out of memory: Killed process 1234 (my_app) total-vm:204800kB, anon-rss:150000kB, file-rss:0kB";
    assert_eq!(
        match_message(msg),
        Some(EventKind::OomKill {
            pid: 1234,
            comm: "my_app".into(),
        })
    );
}

#[test]
fn oom_task_pattern_memcg() {
    let msg = "Memory cgroup out of memory: Killed process 999 (dbus-daemon) total-vm:123456kB, anon-rss:45678kB, file-rss:0kB, shmem-rss:0kB, UID:1000 pgtables:1024kB oom_score_adj:0";
    assert_eq!(
        match_message(msg),
        Some(EventKind::OomKill {
            pid: 999,
            comm: "dbus-daemon".into(),
        })
    );
}

#[test]
fn oom_with_task_field() {
    // Neuere Kernel fuegen task=<comm>,pid=<pid>,uid=<uid> hinzu
    let msg = "Out of memory: Killed process 5555 (chromium) total-vm:999999kB, anon-rss:500000kB, file-rss:0kB, shmem-rss:0kB, UID:1000 pgtables:2048kB oom_score_adj:300 task=chromium,pid=5555,uid=1000";
    assert_eq!(
        match_message(msg),
        Some(EventKind::OomKill {
            pid: 5555,
            comm: "chromium".into(),
        })
    );
}

// --- NVIDIA Xid -----------------------------------------------------------

#[test]
fn xid_31_page_fault() {
    let msg = "NVRM: Xid (PCI:0000:03:00): 31, pid='1234', name='python3', 'Illegal memory access'";
    assert_eq!(
        match_message(msg),
        Some(EventKind::GpuXid {
            code: 31,
            pci: Some("0000:03:00".into()),
            pid: Some(1234),
            message: msg.into(),
        })
    );
}

#[test]
fn xid_79_fallen_off_bus() {
    let msg = "NVRM: Xid (PCI:0000:01:00:0): 79, GPU has fallen off the bus.";
    assert_eq!(
        match_message(msg),
        Some(EventKind::GpuXid {
            code: 79,
            pci: Some("0000:01:00:0".into()),
            pid: None,
            message: msg.into(),
        })
    );
}

#[test]
fn xid_62_no_pci() {
    // Treiberversionen ohne PCI-Angabe: "NVRM: Xid: 62, ..."
    let msg = "NVRM: Xid: 62, Internal micro-controller halt.";
    assert_eq!(
        match_message(msg),
        Some(EventKind::GpuXid {
            code: 62,
            pci: None,
            pid: None,
            message: msg.into(),
        })
    );
}

#[test]
fn xid_pid_unquoted_variant() {
    // Aeltere Treiberlog-Variante: "pid=1234" ohne Quoting (Review 2.4)
    let msg = "NVRM: Xid (PCI:0000:03:00): 13, pid=1234, name=python3, Graphics Exception";
    assert_eq!(
        match_message(msg),
        Some(EventKind::GpuXid {
            code: 13,
            pci: Some("0000:03:00".into()),
            pid: Some(1234),
            message: msg.into(),
        })
    );
}

#[test]
fn xid_43_without_pci_prefix() {
    // Variante "NVRM: Xid (0000:03:00): 43" ohne "PCI:"-Praeﬁx
    let msg = "NVRM: Xid (0000:03:00): 43, GPU stopped processing";
    assert_eq!(
        match_message(msg),
        Some(EventKind::GpuXid {
            code: 43,
            pci: Some("0000:03:00".into()),
            pid: None,
            message: msg.into(),
        })
    );
}

#[test]
fn xid_severity_table() {
    assert_eq!(xid_severity(13), "high");
    assert_eq!(xid_severity(31), "high");
    assert_eq!(xid_severity(43), "high");
    assert_eq!(xid_severity(45), "high");
    assert_eq!(xid_severity(62), "critical");
    assert_eq!(xid_severity(79), "fatal");
    assert_eq!(xid_severity(999), "unknown");
}

// --- amdgpu ---------------------------------------------------------------

#[test]
fn amdgpu_reset_begin() {
    let msg = "amdgpu: GPU reset begin!";
    assert_eq!(
        match_message(msg),
        Some(EventKind::GpuReset {
            vendor: "amdgpu".into(),
            detail: msg.into(),
        })
    );
}

#[test]
fn amdgpu_reset_succeeded() {
    let msg = "amdgpu: GPU reset(3) succeeded!";
    assert_eq!(
        match_message(msg),
        Some(EventKind::GpuReset {
            vendor: "amdgpu".into(),
            detail: msg.into(),
        })
    );
}

#[test]
fn amdgpu_reset_failed() {
    let msg = "amdgpu: GPU reset(3) failed";
    assert_eq!(
        match_message(msg),
        Some(EventKind::GpuReset {
            vendor: "amdgpu".into(),
            detail: msg.into(),
        })
    );
}

#[test]
fn amdgpu_ring_timeout() {
    let msg = "amdgpu: *ERROR* ring gfx_0.0.0 timeout, signaled seq=1, emitted seq=2";
    assert_eq!(
        match_message(msg),
        Some(EventKind::GpuReset {
            vendor: "amdgpu".into(),
            detail: msg.into(),
        })
    );
}

// --- Intel i915 / xe ------------------------------------------------------

#[test]
fn i915_resetting_chip() {
    let msg = "i915: Resetting chip after gpu hang";
    assert_eq!(
        match_message(msg),
        Some(EventKind::GpuReset {
            vendor: "i915".into(),
            detail: msg.into(),
        })
    );
}

#[test]
fn i915_gpu_hang_engine_reset() {
    let msg = "GPU HANG: eGPU 0:0:0, 0x00000000, context 123, command streamer reset";
    assert_eq!(
        match_message(msg),
        Some(EventKind::GpuReset {
            vendor: "i915".into(),
            detail: msg.into(),
        })
    );
}

#[test]
fn xe_wedged() {
    let msg =
        "xe 0000:00:02.0: [drm] *ERROR* CRITICAL: Xe has declared device 0000:00:02.0 as wedged.";
    assert_eq!(
        match_message(msg),
        Some(EventKind::GpuWedged { method: None })
    );
}

// --- Coredump (Felder statt MESSAGE) --------------------------------------

#[test]
fn coredump_fields() {
    let fields = [
        ("MESSAGE_ID", MESSAGE_ID_COREDUMP),
        ("COREDUMP_PID", "552351"),
        ("COREDUMP_UID", "1000"),
        ("COREDUMP_SIGNAL", "11"),
        ("COREDUMP_SIGNAL_NAME", "SIGSEGV"),
        ("COREDUMP_COMM", "firefox"),
        ("COREDUMP_EXE", "/usr/lib64/firefox/firefox"),
        (
            "COREDUMP_CMDLINE",
            "/usr/lib64/firefox/firefox -contentproc -childID 5",
        ),
        (
            "COREDUMP_FILENAME",
            "/var/lib/systemd/coredump/core.firefox.552351.zst",
        ),
        ("COREDUMP_UNIT", "app-gnome-firefox.scope"),
    ];
    assert_eq!(
        parse_coredump(&fields),
        Some(EventKind::Coredump {
            pid: 552351,
            exe: Some("/usr/lib64/firefox/firefox".into()),
            comm: "firefox".into(),
            signal: Some("SIGSEGV".into()),
            uid: Some(1000),
            unit: Some("app-gnome-firefox.scope".into()),
            coredump_file: Some("/var/lib/systemd/coredump/core.firefox.552351.zst".into()),
        })
    );
}

#[test]
fn coredump_partial_record_optionals_none() {
    // Ohne EXE/SIGNAL_NAME/UID/UNIT/FILENAME bleiben die Optionals None.
    let fields = [
        ("MESSAGE_ID", MESSAGE_ID_COREDUMP),
        ("COREDUMP_PID", "42"),
        ("COREDUMP_COMM", "app"),
    ];
    assert_eq!(
        parse_coredump(&fields),
        Some(EventKind::Coredump {
            pid: 42,
            exe: None,
            comm: "app".into(),
            signal: None,
            uid: None,
            unit: None,
            coredump_file: None,
        })
    );
}

#[test]
fn coredump_missing_pid_is_dropped() {
    let fields = [("MESSAGE_ID", MESSAGE_ID_COREDUMP)];
    assert_eq!(parse_coredump(&fields), None);
}

#[test]
fn wrong_message_id_is_not_coredump() {
    let fields = [("MESSAGE_ID", "deadbeef000000000000000000000000")];
    assert_eq!(parse_coredump(&fields), None);
}

// --- Weitere Kernel-Formate ----------------------------------------------

#[test]
fn oom_reaper_line() {
    let msg = "oom_reaper: reaped process 1234 (my_app)";
    assert_eq!(
        match_message(msg),
        Some(EventKind::OomKill {
            pid: 1234,
            comm: "my_app".into(),
        })
    );
}

#[test]
fn amdgpu_ring_sdma_timeout() {
    let msg = "amdgpu: *ERROR* ring sdma1 timeout, signaled seq=7, emitted seq=8";
    assert_eq!(
        match_message(msg),
        Some(EventKind::GpuReset {
            vendor: "amdgpu".into(),
            detail: msg.into(),
        })
    );
}

#[test]
fn amdgpu_ring_comp_timeout() {
    let msg = "amdgpu: *ERROR* ring comp_1.0.0 timeout";
    assert_eq!(
        match_message(msg),
        Some(EventKind::GpuReset {
            vendor: "amdgpu".into(),
            detail: msg.into(),
        })
    );
}

#[test]
fn i915_reset_begin_line() {
    // i915 emittiert ebenfalls "GPU reset begin" — Vendor muss i915 sein
    let msg = "i915 0000:00:02.0: [drm] GPU reset begin";
    assert_eq!(
        match_message(msg),
        Some(EventKind::GpuReset {
            vendor: "i915".into(),
            detail: msg.into(),
        })
    );
}

// --- Negativfaelle --------------------------------------------------------

#[test]
fn irrelevant_message_is_none() {
    assert_eq!(match_message("pacman: upgraded rust (1.0 -> 1.1)"), None);
    assert_eq!(
        match_message("systemd: Reached target Graphical Interface."),
        None
    );
}

#[test]
fn userspace_task_pattern_is_not_oom() {
    // Guard: "task=comm,pid=N" ohne OOM-Kontext darf KEIN OomKill erzeugen
    let msg = "unit: task=cleanup,pid=1234,uid=1000 finished";
    assert_eq!(match_message(msg), None);
}

#[test]
fn amdgpu_page_fault_is_not_gpu_reset() {
    // Behebbare Page-Faults sind keine Abstuerze (Review-Entscheid,
    // dokumentiert in src/gpu/matcher.rs)
    let msg = "amdgpu: [gfxhub] page fault: src_engine UNF1, fault address 0xdeadbeef";
    assert_eq!(match_message(msg), None);
}

#[test]
fn bare_gpu_reset_without_vendor_is_none() {
    // Ohne Treiber-Praefix ist der Vendor nicht bestimmbar — kein Match
    let msg = "[drm] GPU reset begin";
    assert_eq!(match_message(msg), None);
}
