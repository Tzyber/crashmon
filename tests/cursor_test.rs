//! Cursor-Persistenz (Phase 2.2, TDD).
//!
//! Format: zwei Zeilen — Zeile 1 Coredump-Handle, Zeile 2 Kernel-Handle.
//! Atomarer Write (temp + rename), damit ein Daemon-Crash beim Schreiben
//! nie eine halbe Cursor-Datei hinterlaesst (Plan-Design 2).

use crash_daemon::ingest::cursor::{load, save};
use std::fs;

const CURSOR_1: &str = "s=b99bc0e9197b450c9955a6f6724827f7;i=3627cd;b=91a858220d64493eb233f1713123ae7a;m=803c9970;t=65878757d15ef;x=6bc15ac37fc69f84";
const CURSOR_2: &str = "s=deadbeef;i=42;b=abcd;m=1234;t=5678;x=9abc";

fn temp_file(name: &str) -> std::path::PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!(
        "crashmon-cursor-test-{name}-{}",
        std::process::id()
    ));
    p
}

#[test]
fn save_then_load_roundtrip() {
    // Arrange
    let path = temp_file("roundtrip");
    let _ = fs::remove_file(&path);

    // Act
    save(&path, Some(CURSOR_1), Some(CURSOR_2)).expect("save ok");

    // Assert
    let (c1, c2) = load(&path).expect("load ok");
    assert_eq!(c1.as_deref(), Some(CURSOR_1));
    assert_eq!(c2.as_deref(), Some(CURSOR_2));
    fs::remove_file(&path).ok();
}

#[test]
fn missing_file_loads_none() {
    // Arrange
    let path = temp_file("missing");
    let _ = fs::remove_file(&path);

    // Act + Assert: keine Datei = beide Cursor None, kein Fehler
    let (c1, c2) = load(&path).expect("missing file is not an error");
    assert_eq!(c1, None);
    assert_eq!(c2, None);
}

#[test]
fn corrupt_file_is_reported() {
    // Arrange: Datei mit Muell (z. B. abgebrochener Write eines anderen Tools)
    let path = temp_file("corrupt");
    fs::write(&path, b"\x00\xff garbage ohne newline").expect("write fixture");

    // Act + Assert: korrupte Datei ist ein Fehler, kein stiller None
    let err = load(&path).expect_err("corrupt file must error");
    assert!(err.to_string().contains("UTF-8"), "err = {err}");
    fs::remove_file(&path).ok();
}

#[test]
fn single_line_file_fills_coredump_slot() {
    // Arrange: nur eine Zeile (z. B. Datei aus einer Ein-Handle-Version):
    // Zeile 1 = Coredump-Slot, Kernel-Slot fehlt = None
    let path = temp_file("partial");
    let _ = fs::remove_file(&path);
    fs::write(&path, format!("{CURSOR_2}\n")).expect("write fixture");

    // Act
    let (c1, c2) = load(&path).expect("partial file ok");

    // Assert
    assert_eq!(c1.as_deref(), Some(CURSOR_2), "Zeile 1 = Coredump-Slot");
    assert_eq!(c2, None, "fehlende zweite Zeile = None");
    fs::remove_file(&path).ok();
}

#[test]
fn empty_coredump_slot_with_kernel_cursor() {
    // Arrange: erster Start, Coredump-Handle hatte noch keinen Cursor —
    // leerer Slot Zeile 1, Kernel-Slot Zeile 2 besetzt
    let path = temp_file("firstboot");
    let _ = fs::remove_file(&path);
    fs::write(&path, format!("\n{CURSOR_2}\n")).expect("write fixture");

    // Act + Assert
    let (c1, c2) = load(&path).expect("mixed file ok");
    assert_eq!(c1, None);
    assert_eq!(c2.as_deref(), Some(CURSOR_2));
    fs::remove_file(&path).ok();
}

#[test]
fn empty_cursor_strings_are_none() {
    // Arrange: leere Cursor ("" oder nur whitespace) wuerden set_cursor
    // beim Start brechen — deshalb als None laden
    let path = temp_file("empty");
    let _ = fs::remove_file(&path);
    fs::write(&path, "\n\n").expect("write fixture");

    // Act + Assert
    let (c1, c2) = load(&path).expect("empty lines ok");
    assert_eq!(c1, None);
    assert_eq!(c2, None);
    fs::remove_file(&path).ok();
}

#[test]
fn save_is_atomic() {
    // Arrange
    let path = temp_file("atomic");
    let _ = fs::remove_file(&path);
    let tmp = path.with_extension("tmp");

    // Act: speichern, dann prüfen dass keine tmp-Datei zurueckbleibt
    save(&path, Some(CURSOR_1), None).expect("save ok");

    // Assert
    assert!(!tmp.exists(), "tmp-Datei muss nach rename entfernt sein");
    let (c1, c2) = load(&path).expect("load ok");
    assert_eq!(c1.as_deref(), Some(CURSOR_1));
    assert_eq!(c2, None);
    fs::remove_file(&path).ok();
}
