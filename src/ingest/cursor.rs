//! Cursor-Persistenz (Plan-Design 2).
//!
//! Zwei Zeilen: Zeile 1 = Coredump-Handle, Zeile 2 = Kernel-Handle.
//! Write atomar (temp + rename) — ein Absturz beim Schreiben darf nie eine
//! halbe Datei hinterlassen; der naechste Start wuerde sonst eine kurze
//! Luecke ueberspringen (Cursor hinter dem tatsaechlichen Stand).

use std::fs;
use std::io;
use std::io::Write;
use std::path::Path;

/// Laedt beide Cursor. Fehlende Datei oder leere Zeilen = `None`
/// (frischer Start, kein Fehler). Korrupte Datei = `io::Error`.
pub fn load(path: &Path) -> io::Result<(Option<String>, Option<String>)> {
    let raw = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok((None, None)),
        Err(e) => return Err(e),
    };
    let mut lines = raw.lines();
    let c1 = lines
        .next()
        .filter(|s| !s.trim().is_empty())
        .map(str::to_owned);
    let c2 = lines
        .next()
        .filter(|s| !s.trim().is_empty())
        .map(str::to_owned);
    Ok((c1, c2))
}

/// Speichert beide Cursor atomar. `None`-Seite wird als leere Zeile
/// geschrieben (bestimmt gespeichert, nicht "vergessen").
///
/// W5-Fix (wie output.rs): sync_all vor rename + Verzeichnis-sync —
/// die Resume-Position muss einen Stromausfall ueberleben (ein leerer
/// Cursor waere sonst ein stiller Replay-Verlust).
pub fn save(path: &Path, coredump: Option<&str>, kernel: Option<&str>) -> io::Result<()> {
    let content = format!(
        "{}\n{}\n",
        coredump.unwrap_or_default(),
        kernel.unwrap_or_default()
    );
    let tmp = path.with_extension("tmp");
    let mut f = fs::File::create(&tmp)?;
    f.write_all(content.as_bytes())?;
    f.sync_all()?;
    drop(f);
    fs::rename(&tmp, path)?;
    if let Some(dir) = path.parent()
        && let Ok(d) = fs::File::open(dir)
    {
        let _ = d.sync_all();
    }
    Ok(())
}
