//! Report-Verzeichnis scannen (pure, testbar).
//!
//! Liefert alle lesbaren `crash-<ts>.json`-Reports + Zaehler unlesbarer
//! Dateien. Der Aufrufer (app.rs) merged per ts in seine BTreeMap — nur
//! neue ts werden uebernommen, gleiche ts idempotent ersetzt (Daemon-
//! Semantik: gleiche ts = Replay).

use crash_daemon::output::Report;
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

/// Ergebnis eines Scans.
pub struct ScanOutput {
    /// ts -> Report (nach ts sortiert).
    pub reports: BTreeMap<u64, Report>,
    /// Anzahl unlesbarer/korrupter crash-*.json-Dateien.
    pub corrupt: u64,
}

/// Scannt `dir` auf `crash-<ts>.json`-Dateien.
///
/// W1-Fix: `known` enthaelt die ts, die der Aufrufer bereits parsiert hat —
/// deren Dateien werden NICHT geoeffnet (Kosten: O(n Dirents) statt
/// O(n Dateien) pro Scan; der ts steht im Dateinamen).
/// Fehlendes Verzeichnis: leerer Scan, kein Fehler.
pub fn scan_dir(dir: &Path, known: &HashSet<u64>) -> ScanOutput {
    let mut out = ScanOutput {
        reports: BTreeMap::new(),
        corrupt: 0,
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(ts) = name
            .strip_prefix("crash-")
            .and_then(|s| s.strip_suffix(".json"))
            .and_then(|s| s.parse::<u64>().ok())
        else {
            continue;
        };
        if known.contains(&ts) {
            continue; // schon bekannt — nicht erneut oeffnen/parsen
        }
        match std::fs::read_to_string(entry.path())
            .and_then(|raw| serde_json::from_str::<Report>(&raw).map_err(std::io::Error::other))
        {
            Ok(report) => {
                out.reports.insert(ts, report);
            }
            Err(_) => out.corrupt += 1,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crash_daemon::event::{CrashEvent, EventKind};
    use std::fs;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("crashmon-gui-scan-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).expect("temp dir");
        p
    }

    fn write_report(dir: &Path, ts: u64, kind: EventKind) {
        let report = Report {
            ts,
            cause: CrashEvent { ts, kind },
            related: vec![],
            lost_events: 0,
        };
        let json = serde_json::to_string(&report).expect("json");
        fs::write(dir.join(format!("crash-{ts}.json")), json).expect("write");
    }

    #[test]
    fn scans_only_crash_json() {
        let dir = temp_dir("basic");
        write_report(
            &dir,
            1_000_000,
            EventKind::OomKill {
                pid: 1,
                comm: "a".into(),
            },
        );
        write_report(
            &dir,
            2_000_000,
            EventKind::GpuWedged {
                method: Some("rebind".into()),
                device: None,
            },
        );
        // Fremddateien interessieren nicht
        fs::write(dir.join("config.toml"), "x").unwrap();
        fs::write(dir.join("crashmon-daemon.log"), "y").unwrap();

        let out = scan_dir(&dir, &HashSet::new());
        assert_eq!(out.reports.len(), 2);
        assert_eq!(out.corrupt, 0);
        assert_eq!(
            out.reports.keys().copied().collect::<Vec<_>>(),
            vec![1_000_000, 2_000_000]
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn known_ts_skips_file_open() {
        // W1: bekannte ts werden nicht erneut geparst (nur die Dirents
        // zaehlen) — Datei darf sogar unlesbar geworden sein.
        let dir = temp_dir("known");
        write_report(
            &dir,
            1_000_000,
            EventKind::OomKill {
                pid: 1,
                comm: "a".into(),
            },
        );
        write_report(
            &dir,
            2_000_000,
            EventKind::GpuWedged {
                method: None,
                device: None,
            },
        );
        let mut known = HashSet::new();
        known.insert(1_000_000);
        let out = scan_dir(&dir, &known);
        assert_eq!(out.reports.len(), 1, "nur unbekannte ts geliefert");
        assert!(out.reports.contains_key(&2_000_000));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn corrupt_file_counts() {
        let dir = temp_dir("corrupt");
        write_report(
            &dir,
            1_000_000,
            EventKind::OomKill {
                pid: 1,
                comm: "a".into(),
            },
        );
        fs::write(dir.join("crash-9999999999999.json"), "garbage").unwrap();

        let out = scan_dir(&dir, &HashSet::new());
        assert_eq!(out.reports.len(), 1, "gueltiger Report bleibt");
        assert_eq!(out.corrupt, 1, "Muell-Datei zaehlt");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_dir_is_empty() {
        let out = scan_dir(Path::new("/nonexistent-crashmon-dir"), &HashSet::new());
        assert!(out.reports.is_empty());
        assert_eq!(out.corrupt, 0);
    }
}
