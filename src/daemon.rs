//! Daemon-Wiring (Phase 2.5): Loops + Shutdown-Sequenz.
//!
//! Drei Tasks auf der current_thread-Runtime (LocalSet):
//! 1. Journal-Loop: `JournalSource` (AsyncFd, epoll-Park) -> Events
//! 2. Uevent-Loop: Netlink-Polling (150 ms, WEDGED ist nicht zeitkritisch)
//! 3. Aggregator-Loop: Korrelation + atomare Reports + 5-s-Timer
//!
//! Shutdown (Plan-Design 7): SIGTERM/SIGINT -> watch-Signal -> Reader-Tasks
//! beenden sich nach der aktuellen Runde -> Sender gedroppt -> Aggregator
//! drainet die verbleibenden Kanal-Events (Review 2.4 NOTE: erst leeren,
//! DANN flush), schreibt den offenen Report und beendet sich. TimeoutStopSec
//! (10 s, Unit) deckt Haenger ab.

use crate::aggregate::{Aggregator, EventSender, WINDOW};
use crate::config::Config;
use crate::event::CrashEvent;
use crate::gpu::uevent::GpuUeventListener;
use crate::ingest::journal::SdJournalSource;
use crate::ingest::{Drained, JournalSource};
use crate::output::{prune, write_report};
use std::fs::File;
use std::io;
use std::io::Write;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::Duration;
use tokio::sync::watch;
use tracing::{debug, warn};

/// Uevent-Poll-Intervall (Review 2.3-Entscheidung: Polling statt AsyncFd).
const UEVENT_POLL: Duration = Duration::from_millis(150);
/// Slack auf dem Aggregationsfenster, bevor der Timer `flush` stoesst
/// (Events koennen kurz nach Fenster-Ende eintreffen).
const FLUSH_SLACK: Duration = Duration::from_millis(100);
/// "Inaktiv"-Marker fuer den Flush-Timer: `Instant::now() + MAX` wuerde
/// overflowen (tokio-Panik) — 24 h sind praktisch nie erreicht, da jeder
/// push den Timer neu setzt.
const TIMER_INACTIVE: Duration = Duration::from_secs(86_400);
/// Journal-Re-Open-Intervall (Live-Befund 2.6): sd_journal-Watches zeigen
/// auf die beim open() gefundenen Dateien — bei journald-Neustart oder
/// Rotation veralten sie (der inotify-FD feuert nie wieder, der Daemon
/// wird dauerhaft blind). Periodisches Re-Open registriert frische Watches;
/// die Position bleibt ueber die Cursor-Persistenz erhalten.
const JOURNAL_REOPEN: Duration = Duration::from_secs(60);

/// W4 (Single-Instance-Guard): `flock(LOCK_EX|LOCK_NB)` auf `dump_dir/.lock`.
/// Zwei parallele Daemon-Instanzen (GUI-Knopf + systemd-Unit) wuerden sich
/// sonst den Cursor zerlegen (letzter gewinnt, der andere springt zurueck
/// oder vor — verpasste/doppelte Events, gleiche `ts` ueberschreibt Reports).
/// Der Lock-FD lebt bis zum Drop (Ende von `run`) — der Kernel hebt die
/// Sperre auch bei Absturz automatisch auf.
struct InstanceLock {
    _file: File,
}

impl InstanceLock {
    fn acquire(dir: &Path) -> io::Result<Self> {
        let path = dir.join(".lock");
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false) // PID-Hinweis des Besitzers nicht vor flock loeschen
            .open(&path)?;
        // SAFETY: flock ist ein reiner Syscall auf unserem offenen FD.
        let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if ret != 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::WouldBlock {
                // PID-Hinweis fuer die Meldung: der Besitzer schreibt sie
                // nach dem Acquire in die Datei.
                let pid = std::fs::read_to_string(&path)
                    .ok()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .map(|s| format!(" (PID {s})"))
                    .unwrap_or_default();
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    format!("crashmon laeuft bereits{pid} — nur eine Instanz pro dump_dir"),
                ));
            }
            return Err(err);
        }
        // Eigene PID notieren (nur als Diagnose-Hinweis fuer die zweite
        // Instanz; der Lock selbst ist der flock). Nicht truncaten beim
        // Open: sonst wuerde eine wartende zweite Instanz den Hinweis des
        // Besitzers loeschen, bevor sie flock prueft.
        let _ = file.set_len(0);
        let _ = file.write_all(std::process::id().to_string().as_bytes());
        Ok(Self { _file: file })
    }
}

/// Journal-Reader-Task. `wait_readable`-Fehler sind fatal (Trait-Doc):
/// Task beendet sich mit warn — Restart uebernimmt die Service-Unit.
/// Periodisches Re-Open (JOURNAL_REOPEN): siehe Konstante — veraltete
/// sd_journal-Watches (journald-Neustart/Rotation) werden so geheilt.
pub async fn journal_loop(
    src: SdJournalSource,
    cursor_path: PathBuf,
    sender: EventSender,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut src = src;
    let mut reopen_at = tokio::time::Instant::now() + JOURNAL_REOPEN;
    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            _ = tokio::time::sleep_until(reopen_at) => {
                // Cursor sichern, dann frisch oeffnen (sieht aktuelle
                // Dateien; seek_cursor setzt die Position fort).
                if let Err(e) = src.persist() {
                    warn!("journal cursor persist (reopen) fehlgeschlagen: {e}");
                }
                match SdJournalSource::open(&cursor_path) {
                    Ok(new_src) => {
                        debug!("journal neu geoeffnet (Watch-Refresh)");
                        src = new_src;
                    }
                    Err(e) => warn!("journal re-open fehlgeschlagen: {e}"),
                }
                reopen_at = tokio::time::Instant::now() + JOURNAL_REOPEN;
            }
            r = src.wait_readable() => {
                if let Err(e) = r {
                    warn!("journal wait_readable fehlgeschlagen: {e}");
                    break;
                }
            }
        }
        // B1-Fix: drainen bis Exhausted — Nicht-Matches beenden den Drain
        // NICHT mehr (vorher blieb ein amdgpu-Hang mit 20-50 Kernelzeilen
        // bei ~2 Eintraegen pro Minute haengen). BudgetSpent -> yield_now,
        // damit Aggregator/Uevent-Tasks bei Nicht-Match-Fluten weiterlaufen.
        loop {
            match src.next_event() {
                Drained::Event(Ok(ev)) => sender.try_send(ev),
                Drained::Event(Err(e)) => warn!("journal event fehlerhaft: {e}"),
                Drained::BudgetSpent => tokio::task::yield_now().await,
                Drained::Exhausted => break,
            }
        }
        // Leseposition erst NACH dem Konsum sichern (Trait-Vertrag).
        if let Err(e) = src.persist() {
            warn!("journal cursor persist fehlgeschlagen: {e}");
        }
    }
    debug!("journal loop beendet");
}

/// Uevent-Listener-Task (Polling). Nach `Err` Socket neu aufsetzen
/// (Review 2.3-Policy), kurz warten, dann retry.
pub async fn uevent_loop(
    mut listener: GpuUeventListener,
    sender: EventSender,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut interval = tokio::time::interval(UEVENT_POLL);
    loop {
        tokio::select! {
            _ = shutdown.changed() => break,
            _ = interval.tick() => {}
        }
        match listener.read_events() {
            Ok(events) => {
                for ev in events {
                    sender.try_send(ev);
                }
            }
            Err(e) => {
                warn!("uevent read fehlgeschlagen: {e}, socket neu aufsetzen");
                tokio::select! {
                    _ = shutdown.changed() => break,
                    _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                }
                match GpuUeventListener::setup() {
                    Ok(l) => listener = l,
                    Err(e) => warn!("uevent socket re-setup fehlgeschlagen: {e}"),
                }
            }
        }
    }
    debug!("uevent loop beendet");
}

/// Aggregator-Task: Events korrelieren, Reports atomar schreiben.
/// Endet, wenn alle Sender gedroppt sind (Shutdown): erst verbleibende
/// Kanal-Events verarbeiten, dann `flush` + Report (Review 2.4 NOTE).
/// Report-Write-Fehler: loggen und weiter (naechster Report versucht erneut;
/// der verlorene Report ist nicht rekonstruierbar — bewusst akzeptiert).
pub async fn aggregator_loop(
    mut rx: tokio::sync::mpsc::Receiver<CrashEvent>,
    lost: Arc<AtomicU64>,
    dump_dir: PathBuf,
    max_reports: Option<u64>,
    max_age_days: Option<u64>,
) {
    let mut agg = Aggregator::new(lost);
    // Timer fuer offene Gruppen: schlaegt nach WINDOW+SLACK zu, sofern
    // seit dem letzten push kein Report fertig wurde.
    let timer = tokio::time::sleep(TIMER_INACTIVE);
    tokio::pin!(timer);

    // KEIN shutdown-Branch im Select (Review 2.5 MAJOR): der Aggregator
    // terminiert ausschliesslich ueber rx.recv()==None (alle Sender
    // gedroppt). Ein fruehes Abbrechen wuerde in-flight Journal-Events
    // verlieren (try_send -> Closed) und der Cursor stuende trotzdem
    // dahinter — permanenter Verlust.
    loop {
        tokio::select! {
            ev = rx.recv() => match ev {
                Some(ev) => {
                    // push liefert Some, wenn dieses Event die offene
                    // Gruppe schliesst (Review 2.5 CRITICAL: Report wurde
                    // frueher verworfen, Timer invertiert gesetzt).
                    if let Some(report) = agg.push(ev) {
                        write_report_checked(&dump_dir, &report);
                        prune_checked(&dump_dir, max_reports, max_age_days);
                    }
                    // push eroeffnet IMMER eine Gruppe (auch nach Close) —
                    // Timer gilt fuer die neue Gruppe.
                    timer.as_mut().reset(tokio::time::Instant::now() + WINDOW + FLUSH_SLACK);
                }
                None => break, // alle Sender gedroppt -> Drain-Phase
            },
            _ = &mut timer => {
                if let Some(report) = agg.flush() {
                    write_report_checked(&dump_dir, &report);
                    prune_checked(&dump_dir, max_reports, max_age_days);
                }
                timer.as_mut().reset(tokio::time::Instant::now() + TIMER_INACTIVE);
            },
        }
    }

    // Drain-Phase: verbleibende Kanal-Events verarbeiten, dann flush
    // (bei Shutdown koennen noch Events im Kanal liegen).
    while let Ok(ev) = rx.try_recv() {
        if let Some(report) = agg.push(ev) {
            write_report_checked(&dump_dir, &report);
        }
    }
    if let Some(report) = agg.flush() {
        write_report_checked(&dump_dir, &report);
        prune_checked(&dump_dir, max_reports, max_age_days);
    }
    debug!("aggregator loop beendet");
}

fn write_report_checked(dir: &std::path::Path, report: &crate::output::Report) {
    match write_report(dir, report) {
        Ok(path) => debug!("report geschrieben: {}", path.display()),
        Err(e) => warn!("report schreiben fehlgeschlagen: {e}"),
    }
}

/// k4: Retention nach jedem Report-Write — Fehler nur loggen (Rotation
/// ist Nebensache, darf den Daemon nicht stoeren).
fn prune_checked(dir: &Path, max_reports: Option<u64>, max_age_days: Option<u64>) {
    match prune(dir, max_reports, max_age_days) {
        Ok(0) => {}
        Ok(n) => debug!("{n} alte Reports entfernt (Retention)"),
        Err(e) => warn!("retention prune fehlgeschlagen: {e}"),
    }
}

/// Startet die drei Loops im LocalSet und wartet auf SIGTERM/SIGINT.
/// Danach: Shutdown-Signal, alle Tasks sauber beenden (Drain + Flush).
pub async fn run(cfg: Config) -> Result<(), Box<dyn std::error::Error>> {
    let (sender, rx) = crate::aggregate::channel();
    let lost = sender.lost_counter();
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    // dump_dir frueh anlegen: Cursor-Persistenz braucht das Verzeichnis
    // VOR dem ersten Report (Live-Test-Befund 2.5).
    std::fs::create_dir_all(&cfg.dump_dir)?;
    // W4: Single-Instance-Guard — der Lock lebt bis zum Ende von `run`.
    let _lock = InstanceLock::acquire(&cfg.dump_dir)?;
    // Cursor-Persistenz neben den Reports im dump_dir (StateDirectory).
    let cursor_path = cfg.dump_dir.join("cursor");
    let src = SdJournalSource::open(&cursor_path)?;
    let listener = GpuUeventListener::setup()?;

    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;

    let set = tokio::task::LocalSet::new();
    let mut j = set.spawn_local(journal_loop(
        src,
        cursor_path,
        sender.clone(),
        shutdown_rx.clone(),
    ));
    let u = set.spawn_local(uevent_loop(listener, sender, shutdown_rx.clone()));
    let mut a = set.spawn_local(aggregator_loop(
        rx,
        lost,
        cfg.dump_dir.clone(),
        cfg.max_reports,
        cfg.max_age_days,
    ));

    // Warten bis SIGTERM/SIGINT oder eine Task frueh beendet (Fehlerfall).
    // Live-Befund 2.6 (CRITICAL): Der Select MUSS in run_until laufen —
    // spawn_local-Tasks ticken NUR dort. Ausserhalb waere der Daemon
    // eingefroren (keine Events, keine Re-Open-Timer; Reports kaemen nur
    // beim Shutdown-Flush).
    // Review 2.5 HIGH: ohne journal-Branch wuerde ein fruehes Ende des
    // Journal-Loops still degradieren (Uevent-only) und Restart=on-failure
    // nie greifen — der Prozess lebt ja.
    let mut early: Option<&'static str> = None;
    set.run_until(async {
        tokio::select! {
            _ = sigterm.recv() => tracing::info!("SIGTERM empfangen, fahre herunter"),
            _ = sigint.recv() => tracing::info!("SIGINT empfangen, fahre herunter"),
            _ = &mut j => early = Some("journal"),
            _ = &mut a => early = Some("aggregator"),
        }
    })
    .await;
    // Reader stoppen; Aggregator endet ueber Sender-Drop (kein eigener
    // Shutdown-Branch, Review 2.5 MAJOR — in-flight Events gehen nicht
    // verloren).
    let _ = shutdown_tx.send(true);
    set.run_until(async move {
        let _ = j.await;
        let _ = u.await;
        let _ = a.await;
    })
    .await;
    if let Some(who) = early {
        // Non-zero Exit -> systemd Restart=on-failure greift.
        return Err(format!("{who}-task frueh beendet").into());
    }
    tracing::info!("crashmon beendet");
    Ok(())
}
