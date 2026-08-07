//! Output-Tests (Phase 2.4, Spec OU-1/OU-2).
//!
//! JSON-Format + atomarer Write (temp+rename, kein halber Report).

use crash_daemon::event::{CrashEvent, EventKind};
use crash_daemon::output::{Report, write_report};
use std::fs;

fn xid_report() -> Report {
    Report {
        ts: 1_786_126_270_648_827,
        cause: CrashEvent {
            ts: 1_786_126_270_648_827,
            kind: EventKind::GpuXid {
                code: 31,
                pci: Some("0000:03:00".into()),
                pid: Some(1234),
                message: "NVRM: Xid (PCI:0000:03:00): 31, pid='1234'".into(),
            },
        },
        related: vec![CrashEvent {
            ts: 1_786_126_271_000_000,
            kind: EventKind::GpuXid {
                code: 43,
                pci: Some("0000:03:00".into()),
                pid: None,
                message: "NVRM: Xid (PCI:0000:03:00): 43".into(),
            },
        }],
        lost_events: 0,
    }
}

fn temp_dir(name: &str) -> std::path::PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "crashmon-output-test-{name}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).expect("temp dir");
    p
}

#[test]
fn json_format_has_expected_structure() {
    // Arrange
    let report = xid_report();

    // Act: serialisieren wie write_report es tut
    let raw = serde_json::to_vec_pretty(&report).expect("json");
    let json: serde_json::Value = serde_json::from_slice(&raw).expect("valid json");

    // Assert: Struktur laut Spec OU-1 (cause.kind = tag, related-Array, lost)
    let cause = json.get("cause").expect("cause-feld");
    let related = json
        .get("related")
        .expect("related-feld")
        .as_array()
        .unwrap();
    assert_eq!(
        json.get("ts").and_then(|v| v.as_u64()),
        Some(1_786_126_270_648_827)
    );
    assert_eq!(
        cause.get("ts").and_then(|v| v.as_u64()),
        Some(1_786_126_270_648_827)
    );
    assert_eq!(
        cause
            .get("event")
            .and_then(|v| v.get("kind"))
            .and_then(|v| v.as_str()),
        Some("GpuXid")
    );
    assert_eq!(
        cause
            .get("event")
            .and_then(|v| v.get("data"))
            .and_then(|v| v.get("code"))
            .and_then(|v| v.as_u64()),
        Some(31)
    );
    assert_eq!(
        cause
            .get("event")
            .and_then(|v| v.get("data"))
            .and_then(|v| v.get("pid"))
            .and_then(|v| v.as_u64()),
        Some(1234)
    );
    assert_eq!(related.len(), 1);
    assert_eq!(
        related[0]
            .get("event")
            .and_then(|v| v.get("data"))
            .and_then(|v| v.get("code"))
            .and_then(|v| v.as_u64()),
        Some(43)
    );
    assert_eq!(json.get("lost_events").and_then(|v| v.as_u64()), Some(0));
}

#[test]
fn write_report_atomic_no_tmp_leftover() {
    // Arrange
    let dir = temp_dir("atomic");
    let report = xid_report();

    // Act
    let path = write_report(&dir, &report).expect("write ok");

    // Assert: Dateiname crash-<ts>.json, Inhalt parsebar, kein tmp uebrig
    assert_eq!(
        path.file_name().unwrap().to_str().unwrap(),
        format!("crash-{}.json", report.ts)
    );
    let raw = fs::read_to_string(&path).expect("read report");
    let parsed: serde_json::Value = serde_json::from_str(&raw).expect("valid json");
    assert_eq!(
        parsed
            .get("cause")
            .and_then(|v| v.get("event"))
            .and_then(|v| v.get("kind"))
            .and_then(|v| v.as_str()),
        Some("GpuXid")
    );
    assert!(!dir.join(format!("crash-{}.tmp", report.ts)).exists());
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn serialize_deserialize_roundtrip_all_variants() {
    // GUI liest die Reports — Roundtrip fuer alle EventKind-Varianten
    // (fängt tag/content/rename-Fehler, GUI-Plan Schritt 2).
    let kinds = vec![
        EventKind::Coredump {
            pid: 42,
            exe: Some("/usr/bin/app".into()),
            comm: "app".into(),
            signal: Some("SIGSEGV".into()),
            uid: Some(1000),
            unit: Some("user@1000.service".into()),
            coredump_file: Some("/var/lib/systemd/coredump/core.app.zst".into()),
        },
        EventKind::OomKill {
            pid: 7,
            comm: "oomed".into(),
        },
        EventKind::GpuXid {
            code: 31,
            pci: Some("0000:03:00".into()),
            pid: Some(1234),
            message: "NVRM: Xid (PCI:0000:03:00): 31".into(),
        },
        EventKind::GpuReset {
            vendor: "amdgpu".into(),
            detail: "amdgpu: GPU reset begin!".into(),
        },
        EventKind::GpuWedged {
            method: Some("bus-reset".into()),
        },
    ];
    for kind in kinds {
        let report = Report {
            ts: 1_786_126_270_648_827,
            cause: CrashEvent {
                ts: 1_786_126_270_648_827,
                kind,
            },
            related: vec![],
            lost_events: 0,
        };
        let json = serde_json::to_string(&report).expect("serialize");
        let back: Report = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, report, "roundtrip: {json}");
    }
}

#[test]
fn write_report_overwrites_same_ts() {
    // Arrange: gleiche ts (z. B. Neustart-Replay) — alter Report ersetzt
    let dir = temp_dir("overwrite");
    let report = xid_report();

    // Act: zweimal schreiben
    write_report(&dir, &report).expect("first write");
    write_report(&dir, &report).expect("second write");

    // Assert: genau eine Datei, keine tmp-Reste
    let files: Vec<_> = fs::read_dir(&dir)
        .expect("read dir")
        .map(|e| e.unwrap().file_name().to_str().unwrap().to_owned())
        .collect();
    assert_eq!(files, vec![format!("crash-{}.json", report.ts)]);
    fs::remove_dir_all(&dir).ok();
}
