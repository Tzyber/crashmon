//! Konfiguration (TOML).

use serde::Deserialize;
use std::path::PathBuf;

/// Daemon-Konfiguration. Minimal fuer Scaffold; waechst mit den Phasen.
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    /// Zielverzeichnis fuer JSON-Crash-Reports (siehe `output.rs`).
    pub dump_dir: PathBuf,

    #[serde(default = "default_log_level")]
    pub log_level: String,
}

fn default_log_level() -> String {
    "info".into()
}

impl Config {
    /// Liest die TOML-Konfiguration von `path`.
    pub fn load(path: impl AsRef<std::path::Path>) -> Result<Self, Box<dyn std::error::Error>> {
        let raw = std::fs::read_to_string(path)?;
        Ok(toml::from_str(&raw)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_valid_toml() {
        let toml = "dump_dir = \"/tmp/crashmon\"\n";
        let cfg: Config = toml::from_str(toml).expect("valid config");
        assert_eq!(cfg.dump_dir, PathBuf::from("/tmp/crashmon"));
        assert_eq!(cfg.log_level, "info"); // Default greift
    }
}
