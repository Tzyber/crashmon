//! Zustandsverzeichnis + Default-Config fuer den Daemon-Kindprozess.
//!
//! state_dir = $XDG_DATA_HOME/crashmon (Default ~/.local/share/crashmon).
//! Reports, config.toml und crashmon-daemon.log liegen bewusst in EINEM
//! Verzeichnis (leichter zu sichern/löschen) — dokumentierte Abweichung
//! vom strikten XDG-Config-Pfad.

use std::io;
use std::path::{Path, PathBuf};

/// Reiner Aufloeser (testbar): data_home gewinnt, sonst home/.local/share.
fn resolve_state_dir(data_home: Option<&str>, home: Option<&str>) -> PathBuf {
    match data_home {
        Some(d) if !d.is_empty() => PathBuf::from(d).join("crashmon"),
        _ => match home {
            Some(h) if !h.is_empty() => PathBuf::from(h).join(".local/share/crashmon"),
            _ => PathBuf::from("/tmp/crashmon"), // Notfall-Fallback
        },
    }
}

/// state_dir fuer diese Session.
pub fn state_dir() -> PathBuf {
    let data_home = std::env::var("XDG_DATA_HOME").ok();
    let home = std::env::var("HOME").ok();
    resolve_state_dir(data_home.as_deref(), home.as_deref())
}

/// Stellt sicher, dass state_dir + config.toml existieren.
/// Liefert den Config-Pfad. Config wird nur bei Fehlen geschrieben
/// (format!, kein toml-Dep noetig — zwei Felder, deterministisch).
pub fn ensure_default_config(state_dir: &Path) -> io::Result<PathBuf> {
    std::fs::create_dir_all(state_dir)?;
    let config_path = state_dir.join("config.toml");
    if !config_path.exists() {
        let content = format!(
            "# crashmon Konfiguration (von crashmon-gui erzeugt)\n\
             dump_dir = {:?}\n\
             log_level = \"info\"\n",
            state_dir.display().to_string()
        );
        std::fs::write(&config_path, content)?;
    }
    Ok(config_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn resolve_prefers_data_home() {
        let p = resolve_state_dir(Some("/xdg/data"), Some("/home/u"));
        assert_eq!(p, PathBuf::from("/xdg/data/crashmon"));
    }

    #[test]
    fn resolve_falls_back_to_home() {
        let p = resolve_state_dir(None, Some("/home/u"));
        assert_eq!(p, PathBuf::from("/home/u/.local/share/crashmon"));
    }

    #[test]
    fn resolve_last_resort_tmp() {
        assert_eq!(
            resolve_state_dir(None, None),
            PathBuf::from("/tmp/crashmon")
        );
    }

    #[test]
    fn ensure_creates_config_once() {
        let mut dir = std::env::temp_dir();
        dir.push(format!("crashmon-gui-cfg-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);

        let cfg = ensure_default_config(&dir).expect("erste Erzeugung");
        assert!(cfg.exists());
        let raw = fs::read_to_string(&cfg).expect("lesbar");
        assert!(raw.contains("dump_dir"), "{raw}");
        assert!(raw.contains("log_level = \"info\""), "{raw}");

        // Zweiter Aufruf: unveraendert, kein Fehler
        let cfg2 = ensure_default_config(&dir).expect("zweite Erzeugung");
        assert_eq!(fs::read_to_string(cfg2).unwrap(), raw, "idempotent");

        fs::remove_dir_all(&dir).ok();
    }
}
