//! E2E: stirbt die GUI ungeordnet (SIGKILL), bekommt der Daemon per
//! PDEATHSIG SIGTERM und endet — kein Waisen-Prozess.

use std::io::BufRead;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Lebt die PID noch? Zombie (State Z in /proc/<pid>/stat) zaehlt als
/// TOT — ein unreapeter Enkel haengt sonst unter einem Subreaper
/// (cargo/nextest) als Zombie und triebe den Test ins Timeout, obwohl
/// PDEATHSIG korrekt gefeuert hat. kill(pid, 0) sieht Zombies als
/// lebendig — deshalb /proc-Stat statt Signal-Probe.
fn pid_alive(pid: u32) -> bool {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => {
            // Zustand ist das 1. Feld nach dem letzten ')' (comm kann ')'
            // enthalten — deshalb rsplit).
            let state = stat
                .rsplit(')')
                .next()
                .and_then(|s| s.trim().split_whitespace().next())
                .unwrap_or("?");
            state != "Z"
        }
        Err(_) => false, // Prozess weg (auch Zombie-Entry entfernt)
    }
}

#[test]
fn pdeathsig_killt_kind_beim_parent_tod() {
    let mut helper = Command::new(env!("CARGO_BIN_EXE_pdeath_helper"))
        .stdout(Stdio::piped())
        .spawn()
        .expect("helper spawnen");

    let mut line = String::new();
    // read_line kommt von BufRead, nicht Read — BufReader noetig.
    let mut reader = std::io::BufReader::new(helper.stdout.take().unwrap());
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if reader.read_line(&mut line).expect("PID-Zeile lesen") > 0 {
            break;
        }
        assert!(Instant::now() < deadline, "Helper schrieb nie eine PID");
        std::thread::sleep(Duration::from_millis(20));
    }
    let kind_pid: u32 = line.trim().parse().expect("PID parsen");
    // Helper endet selbst (SIGKILL); Kind (sleep) darf NICHT ueberleben.
    let _ = helper.wait();

    // Kind muss innerhalb des Timeouts sterben. Timeout-Pfad raeumt auf:
    // SIGKILL + wait (sonst verfaelscht ein ueberlebendes Kind den naechsten
    // Lauf / haelt Ressourcen).
    let deadline = Instant::now() + Duration::from_secs(10);
    while pid_alive(kind_pid) {
        assert!(
            Instant::now() < deadline,
            "PDEATHSIG feuerte nicht (Timeout) — Kind-PID {kind_pid} lebt"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    // Cleanup: falls doch noch lebend (assert uebersprungen?), killen.
    if pid_alive(kind_pid) {
        unsafe { libc::kill(kind_pid as libc::pid_t, libc::SIGKILL) };
    }
}
