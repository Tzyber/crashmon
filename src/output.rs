//! JSON-Report-Writer (Phase 2.4).
//!
//! `crash-<ts>.json`, ts = UTC-µs der Ursache (nie Lokalzeit, Plan BLOCKER 3).
//! Atomar via temp+rename: ein Absturz beim Schreiben darf keine halbe
//! Report-Datei hinterlassen (gleiches Muster wie Cursor-Persistenz).

use crate::event::CrashEvent;
use serde::Serialize;
use std::fs;
use std::io;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

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
/// W5-Fix: `rename()` garantiert nur Atomizitaet der Sichtbarkeit, nicht
/// der Haltbarkeit — nach Stromausfall kann der Name da sein und der Inhalt
/// leer. Darum: Daten per `sync_all` auf die Platte, dann rename, dann
/// Verzeichniseintrag haltbar machen (best-effort). Kostet ein paar
/// Millisekunden pro Report — bei der Frequenz irrelevant.
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
    let mut f = fs::File::create(&tmp)?;
    f.write_all(&json)?;
    f.sync_all()?; // Daten haltbar, bevor der Name sichtbar wird
    drop(f);
    fs::rename(&tmp, &path)?;
    // Verzeichniseintrag haltbar machen (best-effort — auf manchen
    // Dateisystemen nicht noetig/unterstuetzt).
    if let Ok(d) = fs::File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(path)
}

/// Entfernt alte Reports (k4): haelt hoechstens `max_reports` Dateien und
/// nichts aelter als `max_age_days` (beide optional). Liefert die Anzahl
/// geloeschter Dateien. Best-effort pro Datei: ein Fehler stoppt nicht
/// den Rest (Rotations-Altlasten sollen nicht den Daemon blockieren).
pub fn prune(dir: &Path, max_reports: Option<u64>, max_age_days: Option<u64>) -> io::Result<usize> {
    if max_reports.is_none() && max_age_days.is_none() {
        return Ok(0);
    }
    let now_us = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0);
    let cutoff = max_age_days.map(|d| now_us.saturating_sub(d * 86_400 * 1_000_000));

    let mut files: Vec<(u64, PathBuf)> = Vec::new();
    for entry in fs::read_dir(dir)?.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(ts) = name
            .strip_prefix("crash-")
            .and_then(|s| s.strip_suffix(".json"))
            .and_then(|s| s.parse::<u64>().ok())
        else {
            continue;
        };
        files.push((ts, entry.path()));
    }
    files.sort_by_key(|(ts, _)| *ts);

    let mut removed = 0;
    let mut doomed: Vec<PathBuf> = Vec::new();
    if let Some(max) = max_reports
        && files.len() as u64 > max
    {
        doomed.extend(
            files
                .drain(..(files.len() as u64 - max) as usize)
                .map(|(_, p)| p),
        );
    }
    if let Some(c) = cutoff {
        doomed.extend(files.into_iter().filter(|(ts, _)| *ts < c).map(|(_, p)| p));
    }
    for p in doomed {
        if fs::remove_file(&p).is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}
