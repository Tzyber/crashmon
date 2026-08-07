//! E2E (Phase 2.5): volle Pipeline — Journal -> Matcher -> Aggregator -> Report.
//!
//! Deterministisch: SEGV NACH dem Open injizieren (Position steht am
//! Journal-Ende). Der Report `crash-<ts>.json` muss mit Coredump-Ursache
//! erscheinen. Ignored (braucht Host-Journal + systemd-coredump):
//! `cargo test --test e2e_test -- --ignored --nocapture`.

use crash_daemon::aggregate::channel;
use crash_daemon::daemon::{aggregator_loop, journal_loop};
use crash_daemon::ingest::journal::SdJournalSource;
use std::os::unix::process::ExitStatusExt;
use std::time::Duration;
use tokio::sync::watch;

#[tokio::test(flavor = "current_thread")]
#[ignore]
async fn segv_produces_coredump_report() {
    // ulimit -c unlimited (erbt in den Kindprozess)
    let limit = libc::rlimit {
        rlim_cur: libc::RLIM_INFINITY,
        rlim_max: libc::RLIM_INFINITY,
    };
    assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_CORE, &limit) }, 0);

    let dump_dir = std::env::temp_dir().join(format!("crashmon-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dump_dir);
    std::fs::create_dir_all(&dump_dir).expect("dump dir");

    let cursor_path = dump_dir.join("cursor");
    let src = SdJournalSource::open(&cursor_path).expect("journal open");

    // SEGV ausloesen (Position steht bereits am Ende)
    let status = std::process::Command::new("sh")
        .args(["-c", "kill -SEGV $$"])
        .status()
        .expect("sh starten");
    assert_eq!(
        status.signal(),
        Some(11),
        "Kind an SIGSEGV gestorben: {status}"
    );

    // Pipeline starten
    let (sender, rx) = channel();
    let lost = sender.lost_counter();
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let set = tokio::task::LocalSet::new();
    let j = set.spawn_local(journal_loop(
        src,
        cursor_path.clone(),
        sender,
        shutdown_rx.clone(),
    ));
    let a = set.spawn_local(aggregator_loop(rx, lost, dump_dir.clone()));

    // Bis zu 30 s auf den Report warten (systemd-coredump kann den
    // Journal-Eintrag lastabhaengig um 10-20 s verzoegern)
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    let mut found = false;
    while tokio::time::Instant::now() < deadline {
        set.run_until(async {
            tokio::time::sleep(Duration::from_millis(100)).await;
        })
        .await;
        let files: Vec<_> = std::fs::read_dir(&dump_dir)
            .expect("read dir")
            .map(|e| e.unwrap().file_name().to_str().unwrap().to_owned())
            .filter(|n| n.starts_with("crash-") && n.ends_with(".json"))
            .collect();
        if !files.is_empty() {
            found = true;
            break;
        }
    }

    // Shutdown: Reader stoppen, Aggregator drainen + flushen
    let _ = shutdown_tx.send(true);
    set.run_until(async {
        let _ = j.await;
        let _ = a.await;
    })
    .await;

    assert!(found, "kein crash-<ts>.json innerhalb 30 s erzeugt");

    // Report-Struktur pruefen (cause = Coredump, signal = SIGSEGV)
    let files: Vec<_> = std::fs::read_dir(&dump_dir)
        .expect("read dir")
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("crash-")
        })
        .collect();
    let path = files.first().expect("crash-<ts>.json erzeugt");
    let raw = std::fs::read_to_string(path).expect("report lesbar");
    let json: serde_json::Value = serde_json::from_str(&raw).expect("valid json");
    assert_eq!(
        json.get("cause")
            .and_then(|c| c.get("event"))
            .and_then(|e| e.get("kind"))
            .and_then(|k| k.as_str()),
        Some("Coredump"),
        "Report: {raw}"
    );
    assert_eq!(
        json.get("cause")
            .and_then(|c| c.get("event"))
            .and_then(|e| e.get("data"))
            .and_then(|d| d.get("signal"))
            .and_then(|s| s.as_str()),
        Some("SIGSEGV")
    );
    eprintln!("e2e ok: {}", path.display());

    std::fs::remove_dir_all(&dump_dir).ok();
}
