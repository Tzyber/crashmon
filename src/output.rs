//! JSON-Report-Writer (Phase 2.4).
//!
//! `crash-<ts>.json`, ts = UTC-µs der Ursache (nie Lokalzeit, Plan BLOCKER 3).
//! Atomar via temp+rename: ein Absturz beim Schreiben darf keine halbe
//! Report-Datei hinterlassen (gleiches Muster wie Cursor-Persistenz).

use crate::event::CrashEvent;
use serde::Serialize;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Korrelierter Crash-Report (eine Gruppe, AG-1/AG-2).
/// Deserialize fuer die GUI (liest die Report-Dateien).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct Report {
    /// ts der Ursache, µs UTC-Epoch.
    pub ts: u64,
    /// Fruehester Event der Gruppe = Ursache (T0-Prinzip).
    pub cause: CrashEvent,
    /// Beigeordnete Events (Folge-Xids, korrelierte Coredumps).
    pub related: Vec<CrashEvent>,
    /// Lost-Events-Zaehler — kumulativ seit Daemon-Start (Review 2.4 MINOR:
    /// Semantik bewusst, monotone Gesamtsumme statt Fenster-Delta).
    pub lost_events: u64,
}

/// Schreibt den Report atomar als `crash-<ts>.json` in `dir` (legt das
/// Verzeichnis an). Liefert den finalen Pfad. `serde_json`-Fehler werden
/// als io::Error gemappt (Invariant: korrekt serialisierbare Struct —
/// praktisch nie).
///
/// Invariante (Review 2.4 MINOR): gleiche `ts` ueberschreibt — bei
/// Replay/Neustart gewollt (idempotent), bei einer seltenen µs-Kollision
/// zweier VERSCHIEDENER Reports ein stiller Verlust (akzeptiert).
pub fn write_report(dir: &Path, report: &Report) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let json = serde_json::to_vec_pretty(report)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let path = dir.join(format!("crash-{}.json", report.ts));
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, json)?;
    fs::rename(&tmp, &path)?;
    Ok(path)
}
