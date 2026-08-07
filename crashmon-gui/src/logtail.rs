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
}

impl LogTail {
    pub fn new(max: usize) -> Self {
        Self {
            cursor: 0,
            lines: VecDeque::with_capacity(max),
            max,
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
        }
        if file.seek(SeekFrom::Start(self.cursor)).is_err() {
            return;
        }
        let mut buf = String::new();
        if file.read_to_string(&mut buf).is_err() {
            return;
        }
        self.cursor += buf.len() as u64;
        for line in buf.lines() {
            self.lines.push_back(line.to_owned());
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
}
