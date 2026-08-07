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

/// Severity pro NVIDIA-Xid-Code (Recherche-Tabelle 2.2).
/// 13/31/43/45 = hoch, 62 = kritisch, 79 = fatal.
pub fn xid_severity(code: u16) -> &'static str {
    match code {
        13 | 31 | 43 | 45 => "high",
        62 => "critical",
        79 => "fatal",
        _ => "unknown",
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
        // xe: "Xe has declared device ... as wedged"; WEDGED=<method> kommt
        // nur ueber den Uevent-Pfad (Phase 2.3), nicht ueber MESSAGE.
        return Some(EventKind::GpuWedged { method: None });
    }

    // GPU-Reset-Familie: Vendor ueber Treiber-Praefix bestimmen, nicht ueber
    // Substring — i915 emittiert ebenfalls "[drm] GPU reset begin"-Zeilen.
    let vendor = if message.contains("amdgpu:") {
        Some("amdgpu")
    } else if message.contains("i915") || message.starts_with("GPU HANG") {
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
