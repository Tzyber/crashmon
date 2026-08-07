//! Event-Aggregator: Korrelation + Dedupe (Phase 2.4, Specs AG-1..3).
//!
//! Fenster: 5 s (AG-2). Gruppierung (eine Gruppe = ein Report):
//! - Xid-Burst: gleiche `pci` (oder beide `None`) innerhalb Fenster
//!   -> EIN Report, Ursache = fruehester Xid (T0-Prinzip, AG-1)
//! - Coredump + Xid/OomKill: gleiche `pid` (beide `Some`) im Fenster -> 1 Report
//!
//! Backpressure (AG-3): bounded `mpsc` (Kapazitaet 1024) mit drop-newest —
//! `EventSender::try_send` verwirft bei vollem Kanal das NEUE Event und
//! inkrementiert den Lost-Zaehler (Atomic, landet im Report-Feld).
//!
//! Kern ist synchron testbar (`push`/`flush`); der 5-s-Timer fuer offene
//! Gruppen kommt in Phase 2.5 (Aggregator-Task ruft `flush` nach Inaktivitaet).
//!
//! Bekannte Grenze (Single-Open-Group-Design, Review 2.4): ein Event einer
//! FREMDEN Klasse (z. B. GpuReset waehrend eine Xid-Gruppe offen ist)
//! schliesst die Gruppe auch innerhalb des Fensters. In der Praxis selten —
//! verschiedene Vendor-Familien treten kaum gemischt auf.

use crate::event::{CrashEvent, EventKind};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Korrelationsfenster: Events innerhalb 5 s (AG-2).
pub const WINDOW: Duration = Duration::from_secs(5);
/// Channel-Kapazitaet zwischen Ingest-Tasks und Aggregator (AG-3).
pub const CHANNEL_CAPACITY: usize = 1024;

/// Sender-Seite mit expliziter Ueberlauf-Policy: drop-newest + Lost-Zaehler.
#[derive(Clone)]
pub struct EventSender {
    tx: tokio::sync::mpsc::Sender<CrashEvent>,
    lost: Arc<AtomicU64>,
}

impl EventSender {
    /// Nicht blockierend senden; bei vollem Kanal Event verwerfen und
    /// Lost-Zaehler inkrementieren (Kanal ist bounded, 1024).
    pub fn try_send(&self, event: CrashEvent) {
        if self.tx.try_send(event).is_err() {
            self.lost.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Geteilter Lost-Zaehler (auch vom Aggregator gelesen).
    pub fn lost_counter(&self) -> Arc<AtomicU64> {
        self.lost.clone()
    }
}

/// Kanal zwischen Ingest-Tasks (Journal/Uevent) und Aggregator.
pub fn channel() -> (EventSender, tokio::sync::mpsc::Receiver<CrashEvent>) {
    let (tx, rx) = tokio::sync::mpsc::channel(CHANNEL_CAPACITY);
    (
        EventSender {
            tx,
            lost: Arc::new(AtomicU64::new(0)),
        },
        rx,
    )
}

/// Offene Korrelationsgruppe — wird bei Fenster-Ueberschreitung oder
/// `flush` zu einem Report.
#[derive(Default)]
struct Group {
    events: Vec<CrashEvent>,
}

impl Group {
    /// Passt `ev` in diese Gruppe (Fenster + Klassen-Match)?
    ///
    /// Any-Member-Semantik (Review 2.4, HIGH): geprueft wird gegen JEDES
    /// Gruppenmitglied, nicht nur die Gruppeneroeffnung — sonst brechen
    /// 3-Event-Ketten: Coredump(pid) -> Xid31(pid) -> Xid43(ohne PID) wuerde
    /// Xid43 in eine neue Gruppe werfen (Folge-Xids tragen meist keine PID).
    fn belongs(&self, ev: &CrashEvent) -> bool {
        let Some(cause) = self.events.first() else {
            return true;
        };
        if ev.ts.saturating_sub(cause.ts) > WINDOW.as_micros() as u64 {
            return false;
        }
        self.events
            .iter()
            .any(|m| Self::same_class(&m.kind, &ev.kind))
    }

    /// Gleiche Ereignisklasse (gleiche GPU bzw. gleiche PID)?
    fn same_class(a: &EventKind, b: &EventKind) -> bool {
        match (a, b) {
            // Xid-Burst: gleiche GPU (PCI-ID; beide unbekannt = gleiche GPU,
            // dokumentierter Tradeoff, AG-1)
            (EventKind::GpuXid { pci: x, .. }, EventKind::GpuXid { pci: y, .. }) => x == y,
            // PID-Korrelation: Coredump <-> Xid / OomKill, PID muss in beiden stehen
            (
                EventKind::Coredump { pid: x, .. },
                EventKind::GpuXid { pid: Some(y), .. } | EventKind::OomKill { pid: y, .. },
            ) => x == y,
            (
                EventKind::GpuXid { pid: Some(x), .. } | EventKind::OomKill { pid: x, .. },
                EventKind::Coredump { pid: y, .. },
            ) => x == y,
            _ => false,
        }
    }
}

/// Aggregator: korreliert Events zu Reports. Synchroner Kern, 2.5 treibt ihn.
pub struct Aggregator {
    open: Option<Group>,
    lost: Arc<AtomicU64>,
}

impl Aggregator {
    /// `lost`: geteilter Zaehler vom Kanal (AG-3).
    pub fn new(lost: Arc<AtomicU64>) -> Self {
        Self { open: None, lost }
    }

    /// Fuegt ein Event hinzu. Liefert einen fertigen Report, wenn das
    /// Fenster der offenen Gruppe durch dieses Event geschlossen wird
    /// (AG-INV-3: kein halber Report).
    pub fn push(&mut self, ev: CrashEvent) -> Option<crate::output::Report> {
        let belongs = match &self.open {
            Some(group) => group.belongs(&ev),
            None => false,
        };
        if belongs {
            self.open.as_mut().expect("open").events.push(ev);
            return None;
        }
        // Fenster ueberschritten oder keine Gruppe: alte schliessen,
        // dann neue Gruppe mit diesem Event eroeffnen.
        let closing = self.close();
        if self.open.is_none() {
            self.open = Some(Group { events: vec![ev] });
        }
        closing
    }

    /// Schliesst die offene Gruppe (falls vorhanden): sortiert nach `ts`
    /// (AG-INV-1), erster = Ursache (T0), Rest = related.
    pub fn flush(&mut self) -> Option<crate::output::Report> {
        self.close()
    }

    fn close(&mut self) -> Option<crate::output::Report> {
        let mut group = self.open.take()?;
        group.events.sort_by_key(|e| e.ts);
        let mut events = group.events.into_iter();
        let cause = events.next().expect("Gruppe ist nie leer");
        let related: Vec<_> = events.collect();
        let lost_events = self.lost.load(Ordering::Relaxed);
        Some(crate::output::Report {
            ts: cause.ts,
            cause,
            related,
            lost_events,
        })
    }
}
