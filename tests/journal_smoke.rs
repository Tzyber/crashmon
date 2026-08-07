//! Smoke-Read des Host-Journals (Phase 2.2).
//!
//! Ignored: braucht echtes systemd-Journal + systemd-coredump als
//! core_pattern + Lese-Rechte (wheel/adm/ACL oder systemd-journal-Gruppe).
//! Lokal: `cargo test --test journal_smoke -- --ignored --nocapture`
//!
//! Deterministisch: ein SEGV wird NACH dem Open ausgeloest (Position steht
//! am Journal-Ende), damit das Coredump-Event (MESSAGE_ID-Handle) sicher
//! nach dem Start eintrifft. Verifiziert: Coredump-Handle (Match, Warteschritt,
//! Normalisierung), Cursor-Persistenz. Der Kernel-Handle bleibt hier offen —
//! dessen Matches decken tests/matcher_test.rs deterministisch ab; ein
//! Kernel-Event zur Laufzeit waere nicht deterministisch erzeugbar.

use crash_daemon::event::EventKind;
use crash_daemon::ingest::JournalSource;
use crash_daemon::ingest::journal::SdJournalSource;
use std::os::unix::process::ExitStatusExt;
use std::time::Duration;

#[tokio::test(flavor = "current_thread")]
#[ignore]
async fn smoke_read_host_journal() {
    // Tracing sichtbar machen (warn/debug des Cursor-Pfads)
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_test_writer()
        .try_init();

    // ulimit -c unlimited, damit systemd-coredump ueberhaupt einen Eintrag
    // anlegt (erbt in den Kindprozess).
    let limit = libc::rlimit {
        rlim_cur: libc::RLIM_INFINITY,
        rlim_max: libc::RLIM_INFINITY,
    };
    assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_CORE, &limit) }, 0);

    let cursor_path =
        std::env::temp_dir().join(format!("crashmon-smoke-cursor-{}", std::process::id()));
    let _ = std::fs::remove_file(&cursor_path);

    let mut src = SdJournalSource::open(&cursor_path)
        .expect("journal oeffnen (Leserechte noetig, vgl. Modul-Doc)");
    eprintln!("[smoke] open ok");

    // SEGV ausloesen — ab jetzt ist der Coredump-Eintrag "live" zu erwarten
    let status = std::process::Command::new("sh")
        .args(["-c", "kill -SEGV $$"])
        .status()
        .expect("sh starten");
    assert_eq!(
        status.signal(),
        Some(11),
        "Kind muss an SIGSEGV sterben: {status}"
    );
    eprintln!("[smoke] kill ok");

    // systemd-coredump kann den Journal-Eintrag lastabhaengig um 10-20 s
    // verzoegern — Deadline grosszuegig (ignored-Test, Review 2.5 MINOR).
    let deadline = tokio::time::Instant::now() + Duration::from_secs(35);
    let mut got_coredump = false;
    loop {
        tokio::select! {
            r = src.wait_readable() => r.expect("wait_readable ok"),
            _ = tokio::time::sleep_until(deadline) => break,
        }
        while let Some(ev) = src.next_event() {
            let ev = ev.expect("event parsen/normalisieren");
            eprintln!("smoke event: {ev:?}");
            if matches!(ev.kind, EventKind::Coredump { .. }) {
                got_coredump = true;
            }
        }
        // Trait-Vertrag: Leseposition NACH dem Drain persistieren
        src.persist().expect("cursor persistieren");
        if got_coredump {
            break;
        }
    }
    assert!(
        got_coredump,
        "Coredump-Event nach kill -SEGV nicht empfangen (systemd-coredump aktiv? core_pattern pruefen)"
    );

    // Nach dem ersten wait_readable muss die Cursor-Datei persistiert sein
    let raw = std::fs::read_to_string(&cursor_path).expect("cursor file angelegt");
    assert!(
        raw.lines().next().is_some_and(|l| l.contains(";t=")),
        "coredump-cursor persistiert: {raw:?}"
    );
    std::fs::remove_file(&cursor_path).ok();
}
