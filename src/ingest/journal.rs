//! Journal-Quelle: Prod-Implementierung ueber das `systemd`-Crate =0.10.1.
//!
//! API-Validierung (docs.rs 0.10.1, Phase 2.2 — Abweichungen zum Plan):
//! - `Journal::open` ist deprecated seit 0.8.0 -> `OpenOptions::default().open()`
//! - Cursor-Methoden heissen `cursor()` / `seek_cursor()` (nicht get_/set_)
//! - kein `JournalEntry`-Typ: `next_entry()` liefert `Option<JournalRecord>`
//!   (BTreeMap<String, String>); `JournalSeek::End` existiert nicht
//! - `wait()` blockiert den Thread -> `fd()` + `AsyncFd` + `process()`-Zyklus
//!   (Nop/Append/Invalidate), kein Polling
//! - `Journal` ist `!Send` -> Struct lebt ausschliesslich auf der
//!   current_thread-Task (LocalSet), wird nie ueber Task-Grenzen geteilt
//!
//! Zwei gematchte Handles (Matches sind AND-verknuepft und pro Instanz):
//! - `coredump`: MESSAGE_ID=fc2e22... (systemd-coredump-Records)
//! - `kernel`: _TRANSPORT=kernel (dmesg -> OOM/GPU-Matches)
//!
//! Seek-Semantik (sd_journal): `seek_cursor` + `next()` positioniert auf den
//! Eintrag NACH dem Cursor; `seek(Tail)` ohne Iteration wartet auf den ersten
//! NEUEN Eintrag (sonst Replay des letzten alten). Rotierte Cursor-Eintraege:
//! `next()` liefert 0 statt Fehler -> Start am aktuellen Ende, akzeptiert.

use crate::event::CrashEvent;
use crate::gpu::matcher::{MESSAGE_ID_COREDUMP, match_message, parse_coredump};
use crate::ingest::cursor;
use std::collections::BTreeMap;
use std::io;
use std::os::fd::{FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use systemd::journal::{Journal, JournalSeek, JournalWaitResult, OpenOptions};
use tokio::io::unix::AsyncFd;

/// Prod-Implementierung von `JournalSource` (siehe `super::JournalSource`).
pub struct SdJournalSource {
    coredump: Journal,
    kernel: Journal,
    coredump_fd: AsyncFd<OwnedFd>,
    kernel_fd: AsyncFd<OwnedFd>,
    cursor_path: PathBuf,
    /// Round-Robin: welches Handle `next_event` beim naechsten Aufruf zuerst prueft.
    next_is_coredump: bool,
}

impl SdJournalSource {
    /// Oeffnet beide gematchten Handles (alle Journal-Pfade inkl. /var/log/journal),
    /// stellt Cursor aus `cursor_path` wieder her und registriert die FDs.
    pub fn open(cursor_path: &Path) -> io::Result<Self> {
        // Start-Zeitpunkt VOR dem Open: Events, die waehrend des Oeffnens
        // entstehen (bei grossen Journals dauert das Sekunden), werden per
        // ClockRealtime-Seek trotzdem gesehen (Live-Test-Befund 2.5).
        let start_us = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_micros() as u64)
            .unwrap_or(0);
        let mut coredump = OpenOptions::default().open()?;
        coredump.match_add("MESSAGE_ID", MESSAGE_ID_COREDUMP)?;
        let mut kernel = OpenOptions::default().open()?;
        kernel.match_add("_TRANSPORT", "kernel")?;

        // Korrupte Cursor-Datei: Warnen und frisch starten — ein
        // Crash-Monitor muss laufen, auch wenn die Resume-Datei defekt ist.
        let (c_coredump, c_kernel) = match cursor::load(cursor_path) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("cursor-Datei unlesbar, frischer Start: {e}");
                (None, None)
            }
        };
        seek_to(&mut coredump, c_coredump.as_deref(), start_us)?;
        seek_to(&mut kernel, c_kernel.as_deref(), start_us)?;

        let coredump_fd = AsyncFd::new(dup_fd(&coredump)?)?;
        let kernel_fd = AsyncFd::new(dup_fd(&kernel)?)?;
        // sd-journal-Gotcha: der FD wird erst lesbar, wenn mindestens einmal
        // process()/wait() gerufen wurde (erst dann wird der Watch registriert).
        // Sonst bleibt der FD trotz neuer Eintraege stumm.
        drain(&mut coredump)?;
        drain(&mut kernel)?;
        Ok(Self {
            coredump,
            kernel,
            coredump_fd,
            kernel_fd,
            cursor_path: cursor_path.to_path_buf(),
            next_is_coredump: true,
        })
    }
}

/// Dupliziert den Journal-FD mit `F_DUPFD_CLOEXEC`: das Crate besitzt den
/// Original-FD (schliesst ihn beim Drop), `AsyncFd` braucht ein eigenes
/// `OwnedFd`; CLOEXEC verhindert das Leaken in Kindprozesse.
fn dup_fd(journal: &Journal) -> io::Result<OwnedFd> {
    let fd = journal.fd()?;
    // SAFETY: fd ist ein gueltiger, offener FD des Journal-Objekts.
    // F_DUPFD_CLOEXEC liefert >= 0 bei Erfolg, sonst -1 mit errno.
    let duped = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
    if duped < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: duped ist ein frisch duplizierter, CLOEXEC-markierter FD —
    // Ownership liegt ab jetzt bei uns (OwnedFd schliesst ihn beim Drop;
    // das Journal-Original bleibt davon unberuehrt).
    Ok(unsafe { OwnedFd::from_raw_fd(duped) })
}

/// Setzt ein Handle auf die Position hinter dem Cursor bzw. (frisch) auf
/// den Zeitpunkt VOR dem Open (`start_us`).
///
/// Empirie (Phase 2.2/2.5, gegen Host-Journal verifiziert):
/// - `seek(Tail)` ist unbrauchbar: bei leerem/nicht-passendem Journal klebt
///   die Position auf LOCATION_TAIL (sd-journal.c) — `next()` liefert nach
///   jedem Append fuer immer 0; der journalctl-Fallback (Head+next) half,
///   aber Events, die WAeHREND des Open entstanden, liegen vor der
///   Tail-Position und gehen verloren (Live-Test-Befund 2.5: grosses
///   Journal, open dauert Sekunden).
/// - `seek(ClockRealtime{start_us})` loest beides: Position ist eine echte
///   LOCATION_SEEK (nicht sticky, Appends werden gefunden) und Events ab
///   Daemon-Start werden gesehen. `next()` danach positioniert auf den
///   ersten Eintrag >= start_us (bzw. 0 — dann warten wir auf Appends).
fn seek_to(journal: &mut Journal, cursor: Option<&str>, start_us: u64) -> io::Result<()> {
    match cursor {
        Some(c) => {
            journal.seek_cursor(c)?;
            journal.next()?; // Folgeeintrag nach dem Cursor
        }
        None => {
            journal.seek(JournalSeek::ClockRealtime { usec: start_us })?;
            journal.next()?;
        }
    }
    Ok(())
}

/// Normalisierter Zeitstempel: `_SOURCE_REALTIME_TIMESTAMP` (µs, UTC-Epoch),
/// Fallback `_REALTIME_TIMESTAMP`, sonst Empfangszeit (Plan-Design 3).
fn ts_from(record: &BTreeMap<String, String>) -> u64 {
    for key in ["_SOURCE_REALTIME_TIMESTAMP", "_REALTIME_TIMESTAMP"] {
        if let Some(Ok(ts)) = record.get(key).map(|v| v.parse::<u64>()) {
            return ts;
        }
    }
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0)
}

/// Ergebnis eines Eintrag-Leseversuchs (B1: tri-state statt `Ok(None)` fuer
/// zwei verschiedene Zustaende).
enum Entry {
    /// Eintrag gelesen und gematcht.
    Event(CrashEvent),
    /// Eintrag gelesen, matcht kein Muster — Handle hat moeglicherweise
    /// weitere Eintraege, weiterlesen!
    Skipped,
    /// Handle ist am Ende (kein Eintrag mehr da).
    Exhausted,
}

/// Liest den naechsten Eintrag eines Handles und normalisiert ihn.
/// `next_entry()` konsumiert den Eintrag (Position rueckt vor) — kein
/// Seek noetig, der naechste Aufruf liest einfach den Folgeeintrag.
fn entry_event(handle: &mut Journal, kind: HandleKind) -> io::Result<Entry> {
    let Some(record) = handle.next_entry()? else {
        return Ok(Entry::Exhausted);
    };
    let ts = ts_from(&record);
    let kind = match kind {
        HandleKind::Coredump => {
            let fields: Vec<(&str, &str)> = record
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();
            parse_coredump(&fields)
        }
        HandleKind::Kernel => record.get("MESSAGE").and_then(|m| match_message(m)),
    };
    Ok(match kind {
        Some(kind) => Entry::Event(CrashEvent { ts, kind }),
        None => Entry::Skipped,
    })
}

#[derive(Clone, Copy)]
enum HandleKind {
    Coredump,
    Kernel,
}

/// Implementiert den Trait `JournalSource` (siehe `super`).
impl super::JournalSource for SdJournalSource {
    async fn wait_readable(&mut self) -> io::Result<()> {
        // Auf beiden FDs warten; wer auch immer feuert, beide Handles
        // prozessieren (Nop/Append/Invalidate-Zyklus bis Nop).
        tokio::select! {
            r = self.coredump_fd.readable() => { tracing::debug!("journal wake: coredump fd"); r?.clear_ready(); }
            r = self.kernel_fd.readable() => { tracing::debug!("journal wake: kernel fd"); r?.clear_ready(); }
        }
        // Fehlerpolitik (Review 2.2): process()-Fehler sind fatal (Err) —
        // wenn sd_journal-Fehler bleiben, kommen nie Events; Restart uebernimmt
        // Restart=on-failure.
        drain(&mut self.coredump)?;
        drain(&mut self.kernel)?;
        Ok(())
    }

    fn persist(&mut self) -> io::Result<()> {
        // Cursor nach dem Event-Drain persistieren (Plan-Design 2) —
        // erst jetzt steht die Position auf dem letzten konsumierten
        // Eintrag (2.5-Befund: Speichern in wait_readable wuerde den Stand
        // VOR dem Lesen sichern). cursor() gibt Err (-EADDRNOTAVAIL), wenn
        // die Position auf keinem Eintrag steht (noch nie gelesen) — dann
        // die bisherige Seite AUS DER DATEI behalten statt mit leerer Zeile
        // zu ueberschreiben. Gespeichert wird, sobald eine Seite neu ist.
        let (old1, old2) = cursor::load(&self.cursor_path).unwrap_or((None, None));
        let c1 = match self.coredump.cursor() {
            Ok(c) => Some(c),
            Err(e) => {
                tracing::debug!("coredump-Cursor nicht verfuegbar: {e}");
                old1
            }
        };
        let c2 = match self.kernel.cursor() {
            Ok(c) => Some(c),
            Err(e) => {
                tracing::debug!("kernel-Cursor nicht verfuegbar: {e}");
                old2
            }
        };
        if (c1.is_some() || c2.is_some())
            && let Err(e) = cursor::save(&self.cursor_path, c1.as_deref(), c2.as_deref())
        {
            tracing::warn!("Cursor-Persistenz fehlgeschlagen: {e}");
        }
        Ok(())
    }

    fn next_event(&mut self) -> super::Drained {
        // B1-Fix (tri-state): bis zu BUDGET Eintraege pro Aufruf abarbeiten.
        // Nicht-Matches werden uebersprungen (weiterlesen!), statt den
        // Drain abzubrechen. Erst wenn WIRKLICH nichts mehr da ist, kommt
        // Exhausted; BudgetSpent = Eintraege uebrig, aber Zeitscheibe voll.
        for _ in 0..BUDGET {
            match self.step_one() {
                Step::Event(ev) => return super::Drained::Event(Ok(ev)),
                Step::Skipped => continue,
                Step::Exhausted => return super::Drained::Exhausted,
                Step::Err(e) => return super::Drained::Event(Err(e)),
            }
        }
        super::Drained::BudgetSpent
    }
}

/// Pro-Klick-Zustand der Round-Robin-Iteration.
enum Step {
    /// Eintrag gelesen und gematcht.
    Event(CrashEvent),
    /// Eintrag gelesen, matcht kein Muster — weiterlesen!
    Skipped,
    /// Beide Handles sind am Ende.
    Exhausted,
    Err(io::Error),
}

/// Budget pro `next_event`-Aufruf: harte Obergrenze, damit die Drain-
/// Schleife bei einer Nicht-Match-Flut nicht die anderen LocalSet-Tasks
/// (Aggregator, Uevent) aushungert. `BudgetSpent` + `yield_now` im
/// Konsumenten ist der eigentliche Zweck (B1, rev. 2).
const BUDGET: usize = 512;

impl SdJournalSource {
    /// Ein Round-Robin-Schritt: pro Aufruf genau EIN Eintrag vom rotierenden
    /// Start-Handle. `Skipped` = Eintrag gelesen ohne Match — der Konsument
    /// liest weiter; `Exhausted` = BEIDE Handles wirklich leer.
    fn step_one(&mut self) -> Step {
        let (first, first_j, second_j) = if self.next_is_coredump {
            (HandleKind::Coredump, &mut self.coredump, &mut self.kernel)
        } else {
            (HandleKind::Kernel, &mut self.kernel, &mut self.coredump)
        };
        self.next_is_coredump = !self.next_is_coredump;
        let second = match first {
            HandleKind::Coredump => HandleKind::Kernel,
            HandleKind::Kernel => HandleKind::Coredump,
        };
        match entry_event(first_j, first) {
            Ok(Entry::Event(ev)) => return Step::Event(ev),
            Ok(Entry::Skipped) => return Step::Skipped,
            Ok(Entry::Exhausted) => {}
            Err(e) => return Step::Err(e),
        }
        match entry_event(second_j, second) {
            Ok(Entry::Event(ev)) => Step::Event(ev),
            Ok(Entry::Skipped) => Step::Skipped,
            Ok(Entry::Exhausted) => Step::Exhausted,
            Err(e) => Step::Err(e),
        }
    }
}

/// `process()` bis `Nop` — danach sind neue Eintraege iterierbar.
fn drain(journal: &mut Journal) -> io::Result<()> {
    loop {
        if let JournalWaitResult::Nop = journal.process()? {
            return Ok(());
        }
    }
}
