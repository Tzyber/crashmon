//! Normalisierte Ereignistypen des Daemons.
//!
//! Alle Events tragen `ts` (µs, UTC-Epoch) — aus `_SOURCE_REALTIME_TIMESTAMP`,
//! Fallback: Empfangszeit. Korrelation und Report-Dateinamen basieren
//! ausschliesslich auf `ts`, nie auf Aggregator-Empfangszeit (siehe Plan,
//! Review-Fix: Event-Timestamp).

/// Normalisiertes Crash-/GPU-Ereignis.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CrashEvent {
    /// Zeitstempel µs seit Epoch, UTC.
    pub ts: u64,
    /// Als `"event"` serialisiert (lesbareres Report-Schema:
    /// `cause.event.kind` statt doppeltem `kind`-Key auf zwei Ebenen).
    #[serde(rename = "event")]
    pub kind: EventKind,
}

/// Ereignisklassen. Erweiterbar um weitere Varianten (z. B. Hotplug,
/// wenn gefordert — bewusst nicht im Scaffold, YAGNI).
/// Serialisierung: `{"kind": "GpuXid", "data": {...}}` (Report-JSON, 2.4).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", content = "data")]
pub enum EventKind {
    Coredump {
        pid: u32,
        /// Absoluter Binärpfad; `None` wenn Journal ihn nicht führte.
        exe: Option<String>,
        comm: String,
        signal: Option<String>,
        uid: Option<u32>,
        unit: Option<String>,
        coredump_file: Option<String>,
    },
    OomKill {
        pid: u32,
        comm: String,
    },
    GpuXid {
        code: u16,
        pci: Option<String>,
        /// Aus `pid='N'` in der Xid-Message (falls vorhanden) — fuer die
        /// PID-Korrelation mit Coredumps (Phase 2.4, AG-2).
        pid: Option<u32>,
        message: String,
    },
    GpuReset {
        vendor: String,
        detail: String,
    },
    GpuWedged {
        method: Option<String>,
    },
}
