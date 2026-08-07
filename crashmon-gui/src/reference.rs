//! Eingebaute Wissensbasis (Event-Erklaerungen + Xid-Referenz).
//!
//! Datenquelle: recherche-phase1.md (verifizierte Tabelle 2.2: NVIDIA
//! Xid-Codes + Severity). Zusaetzlich ein lokaler, editierbarer
//! Wissensspeicher (`knowledge.md` im state_dir) — der User erweitert ihn
//! fuer neue Fehler/Dinge; die GUI zeigt ihn im Referenz-Block an.

use crash_daemon::event::EventKind;

/// Xid-Code -> (Severity, Beschreibung). EINE Quelle (W6-Fix):
/// crash_daemon::gpu::matcher::xid_info ist die Wahrheit — hier wird nur
/// delegiert, keine eigene Tabelle mehr (die drei Listen drifteten auseinander).
pub fn xid_info(code: u16) -> (String, &'static str) {
    let (severity, desc) = crash_daemon::gpu::matcher::xid_info(code);
    (severity.to_string(), desc)
}

/// Kurz-Erklaerung pro Event-Typ (fuer die Referenz-Ansicht).
pub fn event_explanation(kind: &EventKind) -> &'static str {
    match kind {
        EventKind::Coredump { .. } => {
            "Prozess abgestuerzt (Signal). systemd-coredump hat den Speicherabzug gespeichert \
             (CORE-Pfad). Die Core-Datei kann mit gdb analysiert werden."
        }
        EventKind::OomKill { .. } => {
            "Der Kernel-OOM-Killer hat den Prozess beendet (Speichermangel). Pruefen: \
             free -h, zram/swap, Prozess-Speicherverbrauch."
        }
        EventKind::GpuXid { .. } => {
            "NVIDIA-Treiber meldet einen GPU-Fehler (Xid). Siehe Severity + Beschreibung unten."
        }
        EventKind::GpuReset { .. } => {
            "Der Grafiktreiber setzt die GPU zurueck (hang recovery). Wiederholte Resets deuten \
             auf Treiber- oder Hardware-Probleme."
        }
        EventKind::GpuWedged { .. } => {
            "Der Kernel hat die GPU als dauerhaft haengend (wedged) eingestuft und versucht, \
             sie per bus-reset/rebind/reboot wiederherzustellen."
        }
    }
}

/// Referenz-Zeilen fuer einen Event-Typ (Title -> Text).
pub fn reference_lines(kind: &EventKind) -> Vec<(String, String)> {
    let mut lines = Vec::new();
    lines.push(("Erklaerung".into(), event_explanation(kind).to_string()));
    if let EventKind::GpuXid { code, .. } = kind {
        let (severity, desc) = xid_info(*code);
        lines.push(("Severity".into(), format!("{severity} — {desc}")));
    }
    lines
}

/// Default-Vorlage fuer den lokalen Wissensspeicher (wird bei Erststart
/// geschrieben; der User erweitert sie frei).
///
/// Quelle: Repo-Datei `crashmon-gui/knowledge.md` (via include_str! zur
/// Compile-Zeit) — editierbar + versionierbar. Die Laufzeit-Instanz in
/// ~/.local/share/crashmon wird nie ueberschrieben, aber um fehlende
/// Vorlagen-Sektionen ERWEITERT (siehe `merge_knowledge`).
pub fn knowledge_default() -> String {
    include_str!("../knowledge.md").to_string()
}

/// Merged die Vorlage in die lokale Datei: haengt alle `##`-Sektionen der
/// Vorlage an, deren Ueberschrift in der lokalen Datei fehlt. Eigene
/// Eintraege und Auto-gelerntes bleiben unangetastet. `None` wenn nichts
/// zu tun ist (lokale Datei ist bereits auf dem Stand der Vorlage).
pub fn merge_knowledge(local: &str, template: &str) -> Option<String> {
    let mut additions = String::new();
    for (i, section) in template.split("\n## ").enumerate() {
        // Erstes Stueck ist der Kopf (vor der ersten Sektion); der
        // split konsumiert den "## "-Prefix, die Stuecke sind "Titel\nInhalt".
        if i == 0 {
            continue;
        }
        let title = section.lines().next().unwrap_or_default();
        // Vorhandene Sektion? (Titelvergleich reicht fuer die Merge-Regel)
        if local.contains(&format!("\n## {title}")) || local.starts_with(&format!("## {title}")) {
            continue;
        }
        additions.push_str("\n## ");
        additions.push_str(section);
        additions.push('\n');
    }
    if additions.is_empty() {
        None
    } else {
        Some(format!("{local}{additions}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xid_severities_match_research() {
        assert_eq!(xid_info(31).0, "hoch");
        assert_eq!(xid_info(13).0, "hoch");
        assert_eq!(xid_info(43).0, "hoch");
        assert_eq!(xid_info(45).0, "hoch");
        assert_eq!(xid_info(62).0, "kritisch");
        assert_eq!(xid_info(79).0, "fatal");
        assert_eq!(xid_info(999).0, "unbekannt");
    }

    #[test]
    fn xid_descriptions_present() {
        for code in [13, 31, 43, 45, 62, 79] {
            assert!(!xid_info(code).1.is_empty(), "Xid {code} ohne Beschreibung");
        }
    }

    #[test]
    fn explanation_for_every_kind() {
        let kinds = [
            EventKind::Coredump {
                pid: 1,
                exe: None,
                comm: "a".into(),
                signal: None,
                uid: None,
                unit: None,
                coredump_file: None,
            },
            EventKind::OomKill {
                pid: 1,
                comm: "a".into(),
            },
            EventKind::GpuXid {
                code: 31,
                pci: None,
                pid: None,
                message: "x".into(),
            },
            EventKind::GpuReset {
                vendor: "amdgpu".into(),
                detail: "x".into(),
            },
            EventKind::GpuWedged {
                method: None,
                device: None,
            },
        ];
        for kind in kinds {
            assert!(!event_explanation(&kind).is_empty());
            assert!(!reference_lines(&kind).is_empty());
        }
    }

    #[test]
    fn merge_appends_missing_sections_only() {
        let local = "## Eigene Notiz\nmein Eintrag\n\n## Auto-gelernt (DDG): Xid 42\n- text\n";
        let template =
            "# Kopf\n\n## Xid-Codes\n- 13: hoch\n\n## Eigene Notiz\nVOLL ANDERER INHALT\n";
        let merged = merge_knowledge(local, template).expect("Sektion fehlt");
        // Fehlende Sektion angehaengt
        assert!(merged.contains("## Xid-Codes"));
        assert!(merged.contains("- 13: hoch"));
        // Vorhandene Sektion NICHT ueberschrieben/dupliziert (eigener Inhalt bleibt)
        assert_eq!(merged.matches("## Eigene Notiz").count(), 1);
        assert!(merged.contains("mein Eintrag"));
        assert!(!merged.contains("VOLL ANDERER INHALT"));
        // Auto-gelerntes bleibt
        assert!(merged.contains("## Auto-gelernt (DDG): Xid 42"));
    }

    #[test]
    fn merge_noop_when_up_to_date() {
        let template = knowledge_default();
        let local = template.clone();
        assert_eq!(merge_knowledge(&local, &template), None);
        // User-Erweiterungen sind kein Grund fuer einen Merge
        let extended = format!("{local}\n## Meine Sektion\nneu\n");
        assert_eq!(merge_knowledge(&extended, &template), None);
    }

    #[test]
    fn knowledge_default_has_sections() {
        let d = knowledge_default();
        assert!(d.contains("Xid-Codes"));
        assert!(d.contains("Typische Kernel-/Treiber-Meldungen"));
        assert!(d.contains("Troubleshooting"));
        // Verifizierte Kerninhalte
        assert!(d.contains("GPU reset begin"));
        assert!(d.contains("Out of memory: Killed process"));
        assert!(d.contains("gdb <exe> <core-datei>"));
        // User-Ergaenzungen (Signale, Gaming, Panics, coredumpctl)
        assert!(d.contains("SIGSEGV"));
        assert!(d.contains("SIGABRT"));
        assert!(d.contains("Weitere Xid-Codes"));
        assert!(d.contains("Gaming / Proton / Vulkan"));
        assert!(d.contains("DXVK: Device lost"));
        assert!(d.contains("App-Panics"));
        assert!(d.contains("coredumpctl debug"));
    }
}
