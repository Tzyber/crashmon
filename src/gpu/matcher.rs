//! Muster-Matching von Journal-Nachrichten auf GPU-/OOM-/Coredump-Events.
//!
//! Muster-Tabellen zentral hier pflegen — keine hartcodierten Strings in
//! Tasks. Fixtures: `tests/matcher_test.rs`. Quellen: recherche-phase1.md,
//! Abschnitt 1.2/2.2 (Xid-Severity).
//!
//! Bewusste Nicht-Matches (per Review fixiert):
//! - `amdgpu: [gfxhub] page fault` / `VM_L2_PROTECTION_FAULT`: behebbare
//!   Page-Faults sind keine Abstuerze; fatale Faelle folgen ohnehin auf
//!   `GPU reset begin!`-Zeilen, die gematcht werden.

use crate::event::EventKind;
use regex::Regex;
use std::sync::LazyLock;

/// MESSAGE_ID des systemd-coredump-Events (universell, aus Recherche).
pub const MESSAGE_ID_COREDUMP: &str = "fc2e22bc6ee647b6b90729ab34a250b1";

static RE_OOM_KILLED: LazyLock<Regex> = LazyLock::new(|| {
    // Global-OOM: "Out of memory" (groß), Memcg-OOM: "out of memory" (klein)
    Regex::new(r"(?i)out of memory: Killed process (\d+) \(([^)]+)\)").expect("valid oom regex")
});
static RE_OOM_REAPER: LazyLock<Regex> = LazyLock::new(|| {
    // Nachfolgezeile: Kernel hat Prozess-Erinnerungen zurueckgeholt
    Regex::new(r"oom_reaper: reaped process (\d+) \(([^)]+)\)").expect("valid reaper regex")
});
static RE_XID: LazyLock<Regex> = LazyLock::new(|| {
    // "NVRM: Xid (PCI:0000:03:00): 31, ...", "NVRM: Xid (0000:03:00): 43, ..."
    // oder "NVRM: Xid: 62, ..." (ohne PCI)
    Regex::new(r"NVRM: Xid\s*(?:\(([^)]*)\))?\s*:\s*(\d+)").expect("valid xid regex")
});
static RE_RING_TIMEOUT: LazyLock<Regex> = LazyLock::new(|| {
    // amdgpu-Ringe: gfx_0.0.0, sdma1, comp_1.0.0, uvd, vce0, kiq ...
    Regex::new(r"ring \w[\w.]*\s+timeout").expect("valid ring regex")
});
static RE_XID_PID: LazyLock<Regex> = LazyLock::new(|| {
    // "pid='1234', name='python3'" UND aeltere unquotierte Variante
    // "pid=1234" — Optionale PID fuer die Korrelation (Review 2.4 MINOR)
    Regex::new(r"pid=(?:'(\d+)'|(\d+))").expect("valid xid pid regex")
});
static RE_WEDGED_DEVICE: LazyLock<Regex> = LazyLock::new(|| {
    // PCI-ID aus der Wedge-Deklaration: "Xe has declared device 0000:00:02.0
    // as wedged" (k7: Device mitschneiden).
    Regex::new(r"(?i)device\s+([0-9a-f]{4}:[0-9a-f]{2}:[0-9a-f]{2}\.[0-9a-f])")
        .expect("valid wedged device regex")
});

/// Severity-Klassifikation (W6: eine Quelle fuer matcher, GUI, knowledge.md).
/// Sprache: Deutsch (GUI-Sprache des Projekts).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Fatal,
    Kritisch,
    Hoch,
    Mittel,
    Unbekannt,
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Severity::Fatal => write!(f, "fatal"),
            Severity::Kritisch => write!(f, "kritisch"),
            Severity::Hoch => write!(f, "hoch"),
            Severity::Mittel => write!(f, "mittel"),
            Severity::Unbekannt => write!(f, "unbekannt"),
        }
    }
}

/// Xid-Code -> (Severity, Beschreibung). EINE Tabelle (W6-Fix): frueher drei
/// handgepflegte Listen (matcher.rs EN, reference.rs DE, knowledge.md) mit
/// Abweichungen — jetzt ist diese hier die Wahrheit, GUI + knowledge.md
/// lesen hieraus. Basis: recherche-phase1.md 2.2 + knowledge.md-Ergaenzungen.
pub fn xid_info(code: u16) -> (Severity, &'static str) {
    match code {
        13 => (
            Severity::Hoch,
            "Graphics Engine Exception — GPU-Engine meldet einen Fehler; haeufig nach Treiber-/Speicherproblemen.",
        ),
        31 => (
            Severity::Hoch,
            "Illegal memory access — Kernel/App greift auf ungueltigen GPU-Speicher zu; haeufigste Xid-Ursache.",
        ),
        43 => (
            Severity::Hoch,
            "GPU stopped processing — GPU haelt an; oft Folgexid nach Xid 31.",
        ),
        45 => (
            Severity::Hoch,
            "Preemptive cleanup — Treiber raeumt nach einem Fehler auf; oft Folgexid.",
        ),
        62 => (
            Severity::Kritisch,
            "Internal micro-controller halt — der GPU-Controller haelt an; kritisch, oft Hardware-Defekt.",
        ),
        79 => (
            Severity::Fatal,
            "GPU has fallen off the bus — GPU wird vom PCIe-Bus getrennt; fatal, haeufig Hardware/Stromversorgung.",
        ),
        // Ergaenzungen aus knowledge.md ("Weitere Xid-Codes", User-verifiziert)
        8 => (
            Severity::Mittel,
            "FIFO error / Channel Command Error (Treiber-Haenger).",
        ),
        32 => (
            Severity::Hoch,
            "Invalid Context (Proton/Vulkan/DXVK Kontext-Verlust).",
        ),
        48 => (
            Severity::Kritisch,
            "Double Bit ECC Error (Hardware-RAM-Fehler auf VRAM).",
        ),
        92 => (
            Severity::Hoch,
            "High Temperature / Thermal Event (GPU schuetzt sich vor Ueberhitzung).",
        ),
        109 => (
            Severity::Mittel,
            "CTX Switch Timeout (DXVK/VKD3D Timeout bei Spiel-Szenenwechsel).",
        ),
        _ => (
            Severity::Unbekannt,
            "Unbekannter Xid-Code — Details in der NVIDIA-Dokumentation pruefen.",
        ),
    }
}

/// Severity fuer ALLE Event-Typen (D2: Farbgebung der Oberflaeche).
pub fn event_severity(kind: &EventKind) -> Severity {
    match kind {
        EventKind::GpuXid { code, .. } => xid_info(*code).0,
        EventKind::GpuReset { .. } => Severity::Kritisch,
        EventKind::GpuWedged { .. } => Severity::Fatal,
        EventKind::Coredump { .. } | EventKind::OomKill { .. } => Severity::Hoch,
    }
}

/// Baut ein Coredump-Event aus Journal-Feldern.
/// `None` bei fehlender MESSAGE_ID oder fehlendem/unlesbarem COREDUMP_PID
/// (wird geloggt — Coredump-Records nicht still verlieren).
pub fn parse_coredump(fields: &[(&str, &str)]) -> Option<EventKind> {
    let mut map = std::collections::HashMap::new();
    for (k, v) in fields {
        map.insert(*k, *v);
    }
    if map.get("MESSAGE_ID") != Some(&MESSAGE_ID_COREDUMP) {
        return None;
    }
    let pid = match map.get("COREDUMP_PID").and_then(|v| v.parse::<u32>().ok()) {
        Some(p) => p,
        None => {
            tracing::warn!("coredump record ohne gueltiges COREDUMP_PID, Event verworfen");
            return None;
        }
    };
    let opt = |k: &str| map.get(k).filter(|s| !s.is_empty()).map(|s| s.to_string());
    Some(EventKind::Coredump {
        pid,
        exe: opt("COREDUMP_EXE"),
        comm: opt("COREDUMP_COMM").unwrap_or_default(),
        signal: opt("COREDUMP_SIGNAL_NAME"),
        uid: map.get("COREDUMP_UID").and_then(|v| v.parse().ok()),
        unit: opt("COREDUMP_UNIT"),
        coredump_file: opt("COREDUMP_FILENAME"),
    })
}

/// Matcht eine Journal-`MESSAGE`-Zeile gegen bekannte Muster.
/// Reihenfolge: Xid zuerst (spezifisch), dann OOM, dann GPU-Reset/Wedge.
pub fn match_message(message: &str) -> Option<EventKind> {
    if let Some(caps) = RE_XID.captures(message) {
        return Some(EventKind::GpuXid {
            code: caps
                .get(2)
                .expect("Xid-Code-Gruppe ist fix")
                .as_str()
                .parse()
                .expect("Xid-Code ist numerisch"),
            // "(PCI:0000:03:00)" und "(0000:03:00)" → PCI-ID ohne Prefix
            pci: caps
                .get(1)
                .map(|m| {
                    m.as_str()
                        .strip_prefix("PCI:")
                        .unwrap_or(m.as_str())
                        .to_string()
                })
                .filter(|s| !s.is_empty()),
            // Optionale Prozess-PID aus "pid='N'" oder "pid=N" (Korrelation, AG-2)
            pid: RE_XID_PID
                .captures(message)
                .and_then(|m| m.get(1).or_else(|| m.get(2)))
                .and_then(|m| m.as_str().parse().ok()),
            message: message.to_string(),
        });
    }

    if let Some(caps) = RE_OOM_KILLED.captures(message) {
        // "Killed process <pid> (<comm>)": Gruppe 1 = PID, Gruppe 2 = COMM
        return Some(EventKind::OomKill {
            pid: caps
                .get(1)
                .expect("PID-Gruppe ist fix")
                .as_str()
                .parse()
                .expect("PID ist numerisch"),
            comm: caps
                .get(2)
                .expect("COMM-Gruppe ist fix")
                .as_str()
                .to_string(),
        });
    }

    if let Some(caps) = RE_OOM_REAPER.captures(message) {
        return Some(EventKind::OomKill {
            pid: caps
                .get(1)
                .expect("PID-Gruppe ist fix")
                .as_str()
                .parse()
                .expect("PID ist numerisch"),
            comm: caps
                .get(2)
                .expect("COMM-Gruppe ist fix")
                .as_str()
                .to_string(),
        });
    }

    if message.contains("wedged") {
        // k7: nur mit Treiber-Kontext matchen — "wedged" allein (Userspace,
        // andere Subsysteme) waere zu breit. WEDGED=<method> kommt nur ueber
        // den Uevent-Pfad (Phase 2.3), nicht ueber MESSAGE.
        let driver_ctx = ["xe ", "amdgpu", "i915", "[drm]"]
            .iter()
            .any(|c| message.contains(c));
        if driver_ctx {
            // "Xe has declared device 0000:00:02.0 as wedged" -> PCI-ID
            let device = RE_WEDGED_DEVICE.captures(message).map(|c| {
                c.get(1)
                    .expect("PCI-Gruppe ist fix")
                    .as_str()
                    .to_lowercase()
            });
            return Some(EventKind::GpuWedged {
                method: None,
                device,
            });
        }
    }

    // GPU-Reset-Familie: Vendor ueber Treiber-Namen bestimmen. B2-Fix: ohne
    // Doppelpunkt — die echte Zeile ist "[drm:amdgpu_job_timedout [amdgpu]]
    // *ERROR* ring gfx_0.0.0 timeout", nicht "amdgpu: ...". i915 emittiert
    // ebenfalls "[drm] GPU reset begin"-Zeilen.
    let vendor = if message.contains("amdgpu") {
        Some("amdgpu")
    } else if message.contains("i915") || message.contains("xe ") || message.starts_with("GPU HANG")
    {
        Some("i915")
    } else {
        None
    };
    if let Some(v) = vendor {
        let is_reset = message.contains("GPU reset")
            || message.contains("GPU HANG")
            || message.contains("Resetting chip")
            || RE_RING_TIMEOUT.is_match(message);
        if is_reset {
            return Some(EventKind::GpuReset {
                vendor: v.into(),
                detail: message.to_string(),
            });
        }
    }

    None
}
