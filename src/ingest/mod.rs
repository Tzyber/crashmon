//! Ingestion: systemd-Journal lesen (coredump + kernel).
//!
//! Architektur (Plan-Design 5/8): Zwei gematchte sd_journal-Handles
//! (MESSAGE_ID=Coredump, _TRANSPORT=kernel) in EINER Task — `Journal`
//! ist `!Send` und wird nie ueber Task-Grenzen geteilt. Abstrahiert
//! hinter `JournalSource`, damit Tests einen in-memory-Fake treiben koennen.

pub mod cursor;
pub mod journal;

use crate::event::CrashEvent;

/// Abstraktion ueber das systemd-Journal. Tests verwenden einen Fake,
/// der dieselben Methoden treibt (inkl. Warteschritt).
pub trait JournalSource {
    /// Wartet, bis neue Eintraege lesbar sind (epoll-Park, kein Polling).
    ///
    /// Fehlerpolitik: `Err` ist fatal (FD/process-Fehler) — Konsument soll
    /// sich beenden (Restart uebernimmt die Service-Unit). Cursor-
    /// Persistenz-Fehler werden intern geschluckt (nur warn-Log).
    fn wait_readable(&mut self) -> impl std::future::Future<Output = std::io::Result<()>>;

    /// Liefert den naechsten normalisierten Event, sofern vorhanden.
    ///
    /// Vertrag: nach jedem `wait_readable` MUSS der Konsument bis `None`
    /// drainen — sonst bleiben bereits gepufferte Events liegen, bis der
    /// naechste Journal-Append eintrifft.
    fn next_event(&mut self) -> Option<std::io::Result<CrashEvent>>;

    /// Persistiert die Leseposition (Cursor).
    ///
    /// Vertrag: NACH dem Event-Drain aufrufen — erst dann steht die
    /// Position auf dem letzten konsumierten Eintrag (2.5-Befund: ein
    /// Speichern in `wait_readable` wuerde die Position VOR dem Lesen
    /// sichern — EADDRNOTAVAIL bzw. veralteter Stand). Fehler sind nicht
    /// fatal (nur Resume degradiert), werden intern geloggt.
    fn persist(&mut self) -> std::io::Result<()>;
}
