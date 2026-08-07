//! Cursor-Persistenz (Plan-Design 2).
//!
//! Zwei Zeilen: Zeile 1 = Coredump-Handle, Zeile 2 = Kernel-Handle.
//! Write atomar (temp + rename) — ein Absturz beim Schreiben darf nie eine
//! halbe Datei hinterlassen; der naechste Start wuerde sonst eine kurze
//! Luecke ueberspringen (Cursor hinter dem tatsaechlichen Stand).

use std::fs;
use std::io;
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
pub fn save(path: &Path, coredump: Option<&str>, kernel: Option<&str>) -> io::Result<()> {
    let content = format!(
        "{}\n{}\n",
        coredump.unwrap_or_default(),
        kernel.unwrap_or_default()
    );
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, content)?;
    fs::rename(&tmp, path)
}
