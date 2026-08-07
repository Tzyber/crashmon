//! Ingestion: systemd-Journal lesen (coredump + kernel).
//!
//! Architektur (Plan-Design 5/8): Zwei gematchte sd_journal-Handles
//! (MESSAGE_ID=Coredump, _TRANSPORT=kernel) in EINER Task — `Journal`
//! ist `!Send` und wird nie ueber Task-Grenzen geteilt. Abstrahiert
//! hinter `JournalSource`, damit Tests einen in-memory-Fake treiben koennen.

pub mod cursor;
pub mod journal;

use crate::event::CrashEvent;

/// Ergebnis eines `next_event`-Aufrufs (B1-Fix, tri-state).
///
/// `None` bedeutet zwei verschiedene Dinge („Handle leer" vs. „Eintrag
/// gelesen, aber kein Match") — ein einwertiges `Option` liess die
/// Drain-Schleife beim ersten nicht-matchenden Eintrag abbrechen (stiller
/// Totalausfall unter Nicht-Match-Fluten). Drei Zustaende bis ganz oben:
pub enum Drained {
    /// Ein Event (oder ein Lesefehler) ist zurueck.
    Event(std::io::Result<CrashEvent>),
    /// Eintraege sind noch da, aber das Zeitscheiben-Budget ist verbraucht —
    /// Konsument soll `yield_now` machen und weiter drainen (Fairness gegen
    /// die anderen LocalSet-Tasks bei Nicht-Match-Fluten).
    BudgetSpent,
    /// Wirklich leer: alle Handles sind am Ende.
    Exhausted,
}

/// Abstraktion ueber das systemd-Journal. Tests verwenden einen Fake,
/// der dieselben Methoden treibt (inkl. Warteschritt).
pub trait JournalSource {
    /// Wartet, bis neue Eintraege lesbar sind (epoll-Park, kein Polling).
    ///
    /// Fehlerpolitik: `Err` ist fatal (FD/process-Fehler) — Konsument soll
    /// sich beenden (Restart uebernimmt die Service-Unit). Cursor-
    /// Persistenz-Fehler werden intern geschluckt (nur warn-Log).
    fn wait_readable(&mut self) -> impl std::future::Future<Output = std::io::Result<()>>;

    /// Liefert den naechsten normalisierten Event (tri-state, siehe `Drained`).
    ///
    /// Vertrag: nach jedem `wait_readable` MUSS der Konsument bis
    /// `Drained::Exhausted` drainen — sonst bleiben bereits gepufferte
    /// Events liegen, bis der naechste Journal-Append eintrifft.
    /// `BudgetSpent` heisst: weiterlesen, aber vorher `yield_now()`.
    fn next_event(&mut self) -> Drained;

    /// Persistiert die Leseposition (Cursor).
    ///
    /// Vertrag: NACH dem Event-Drain aufrufen — erst dann steht die
    /// Position auf dem letzten konsumierten Eintrag (2.5-Befund: ein
    /// Speichern in `wait_readable` wuerde die Position VOR dem Lesen
    /// sichern — EADDRNOTAVAIL bzw. veralteter Stand). Fehler sind nicht
    /// fatal (nur Resume degradiert), werden intern geloggt.
    fn persist(&mut self) -> std::io::Result<()>;
}
