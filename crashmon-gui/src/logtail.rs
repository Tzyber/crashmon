//! Cursor-basierter Datei-Tail (Daemon-Log).
//!
//! Merkt sich den Offset und liest nur ab dort — billig bei wachsenden
//! Dateien. Rotation (Datei kleiner als Cursor): Puffer leeren, Cursor 0.

use std::collections::VecDeque;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

pub struct LogTail {
    cursor: u64,
    lines: VecDeque<String>,
    max: usize,
    /// Angebrochene letzte Zeile (ohne `\n`), wartet auf den Abschluss (k5).
    partial: String,
}

impl LogTail {
    pub fn new(max: usize) -> Self {
        Self {
            cursor: 0,
            lines: VecDeque::with_capacity(max),
            max,
            partial: String::new(),
        }
    }

    /// Liest neue Zeilen ab dem letzten Offset. Fehlende Datei: No-op.
    pub fn refresh(&mut self, path: &Path) {
        let Ok(mut file) = std::fs::File::open(path) else {
            return;
        };
        let Ok(len) = file.metadata().map(|m| m.len()) else {
            return;
        };
        if len < self.cursor {
            // Rotation: von vorne
            self.cursor = 0;
            self.lines.clear();
            self.partial.clear();
        }
        if file.seek(SeekFrom::Start(self.cursor)).is_err() {
            return;
        }
        let mut buf = Vec::new();
        if file.read_to_end(&mut buf).is_err() {
            return;
        }
        let consumed = buf.len() as u64;

        // k5: lossy statt read_to_string — eine Nicht-UTF-8-Zeile darf den
        // Tail nicht dauerhaft einfrieren (frueher: Err ohne Cursor-Vorschub
        // -> dieselbe Stelle scheiterte bei jedem Poll).
        //
        // Angebrochene letzte Zeile: erst uebernehmen, wenn das `\n` da ist —
        // sonst wird sie beim naechsten Append als eigene Zeile verdoppelt.
        // Split auf den ROH-Bytes: die lossy-Ersatzzeichen (3 Bytes) wuerden
        // die Laengenrechnung verschieben (u64-Underflow).
        let Some(idx) = buf.iter().rposition(|&b| b == b'\n') else {
            // gar kein Zeilenende im Stueck: alles zuruecksetzen, Text
            // sammelt sich im partial-Puffer.
            self.cursor = self.cursor.saturating_sub(consumed);
            self.partial.push_str(&String::from_utf8_lossy(&buf));
            return;
        };
        // Cursor steht wieder am Anfang der halben Zeile (wird beim
        // naechsten Refresh komplett gelesen).
        let partial_len = (buf.len() - idx - 1) as u64;
        self.cursor = self.cursor + consumed - partial_len;
        self.partial
            .push_str(&String::from_utf8_lossy(&buf[..=idx]));
        for line in std::mem::take(&mut self.partial).lines() {
            self.lines.push_back(strip_ansi(line));
            while self.lines.len() > self.max {
                self.lines.pop_front();
            }
        }
    }

    /// Letzte Zeilen (neueste zuletzt).
    pub fn lines(&self) -> impl Iterator<Item = &str> {
        self.lines.iter().map(String::as_str)
    }
}

/// ANSI-Escapes aus einer Logzeile entfernen. Zweite Verteidigungslinie
/// neben `--with_ansi(false)` im Daemon: Bestandslogs und ein per systemd
/// gestarteter Daemon liefern weiter faerbenden Text, und ESC (0x1B) hat
/// in den egui-Fonts kein Glyph — es erscheint als Tofu-Kaestchen.
fn strip_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.next() {
            // CSI: ESC [ params ... Endbyte im Bereich @..~
            Some('[') => {
                for c in chars.by_ref() {
                    if matches!(c, '\u{40}'..='\u{7e}') {
                        break;
                    }
                }
            }
            // OSC: ESC ] ... BEL (oder ESC \, dann greift der naechste Durchlauf)
            Some(']') => {
                for c in chars.by_ref() {
                    if c == '\u{7}' || c == '\u{1b}' {
                        break;
                    }
                }
            }
            // Einzelzeichen-Escape (ESC c o.ae.): verwerfen.
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_file(name: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("crashmon-gui-tail-{name}-{}", std::process::id()));
        let _ = fs::remove_file(&p);
        p
    }

    #[test]
    fn reads_only_new_lines() {
        let path = temp_file("incr");
        fs::write(&path, "zeile1\nzeile2\n").unwrap();
        let mut tail = LogTail::new(100);
        tail.refresh(&path);
        assert_eq!(tail.lines().collect::<Vec<_>>(), vec!["zeile1", "zeile2"]);

        // Anhaengen: nur die neuen Zeilen kommen
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        use std::io::Write;
        writeln!(file, "zeile3").unwrap();
        tail.refresh(&path);
        assert_eq!(
            tail.lines().collect::<Vec<_>>(),
            vec!["zeile1", "zeile2", "zeile3"]
        );
        fs::remove_file(&path).ok();
    }

    #[test]
    fn rotation_resets() {
        let path = temp_file("rot");
        fs::write(&path, "alt\n").unwrap();
        let mut tail = LogTail::new(100);
        tail.refresh(&path);
        assert_eq!(tail.lines().count(), 1);

        // Datei ersetzt und KÜRZER (Rotation: len < cursor)
        fs::write(&path, "x\n").unwrap();
        tail.refresh(&path);
        assert_eq!(tail.lines().collect::<Vec<_>>(), vec!["x"]);
        fs::remove_file(&path).ok();
    }

    #[test]
    fn strips_ansi_from_lines() {
        // Original-Zeile aus dem Daemon-Log (tracing-subscriber, ANSI an).
        let raw = "\u{1b}[2m2026-08-07T19:55:34.780866Z\u{1b}[0m \u{1b}[32m INFO\u{1b}[0m \
                   \u{1b}[2mcrash_daemon::daemon\u{1b}[0m\u{1b}[2m:\u{1b}[0m beendet";
        assert_eq!(
            strip_ansi(raw),
            "2026-08-07T19:55:34.780866Z  INFO crash_daemon::daemon: beendet"
        );
        // Zeilen ohne Escapes bleiben unveraendert.
        assert_eq!(strip_ansi("nichts zu tun"), "nichts zu tun");
    }

    #[test]
    fn capped_at_max() {
        // Anhaengen wie der echte Daemon-Log (kein Ersetzen!)
        let path = temp_file("cap");
        let mut tail = LogTail::new(3);
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .unwrap();
        use std::io::Write;
        for i in 0..10 {
            writeln!(file, "zeile{i}").unwrap();
            tail.refresh(&path);
        }
        let lines: Vec<_> = tail.lines().collect();
        assert_eq!(lines.len(), 3, "max 3 Zeilen");
        assert_eq!(lines, vec!["zeile7", "zeile8", "zeile9"]);
        fs::remove_file(&path).ok();
    }

    #[test]
    fn missing_file_is_noop() {
        let mut tail = LogTail::new(10);
        tail.refresh(Path::new("/nonexistent-crashmon.log"));
        assert_eq!(tail.lines().count(), 0);
    }

    #[test]
    fn partial_line_waits_for_newline() {
        // k5: "partial" ohne \n wird NICHT als Zeile uebernommen; erst mit
        // dem naechsten Append (der die Zeile abschliesst) erscheint sie —
        // unverdoppelt.
        let path = temp_file("partial");
        fs::write(&path, "zeile1\nangebrochen").unwrap();
        let mut tail = LogTail::new(100);
        tail.refresh(&path);
        assert_eq!(
            tail.lines().collect::<Vec<_>>(),
            vec!["zeile1"],
            "halbe Zeile bleibt unsichtbar"
        );

        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        use std::io::Write;
        write!(file, "e Ende\nzeile3\n").unwrap();
        tail.refresh(&path);
        assert_eq!(
            tail.lines().collect::<Vec<_>>(),
            vec!["zeile1", "angebrochene Ende", "zeile3"]
        );
        fs::remove_file(&path).ok();
    }

    #[test]
    fn non_utf8_does_not_freeze_tail() {
        // k5: Nicht-UTF-8 (z. B. Debug-Binärspucke) darf den Tail nicht
        // dauerhaft stoppen — lossy-Umwandlung, Cursor schreitet voran.
        let path = temp_file("nonutf8");
        fs::write(&path, [b'a', b'b', b'\n', 0xFF, 0xFE]).unwrap();
        let mut tail = LogTail::new(100);
        tail.refresh(&path);
        assert_eq!(tail.lines().count(), 1, "erste Zeile kam durch");

        // Weiteres Append wird trotzdem gelesen (Cursor ist nicht eingefroren)
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        use std::io::Write;
        writeln!(file, "ok").unwrap();
        tail.refresh(&path);
        assert!(
            tail.lines().any(|l| l.ends_with("ok")),
            "Tail liest weiter: {:?}",
            tail.lines().collect::<Vec<_>>()
        );
        fs::remove_file(&path).ok();
    }
}
