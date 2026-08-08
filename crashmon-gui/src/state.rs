//! Daemon-Kindprozess-Lebenszyklus (GUI-Seite).
//!
//! Stop via SIGTERM (libc::kill) — NICHT Child::kill (das waere SIGKILL);
//! der Daemon hat Drain+Flush auf SIGTERM (E2E-verifiziert). Stopping-
//! Deadline (5 s) -> SIGKILL-Fallback. try_wait() reapt (kein Zombie).

use std::io;
use std::os::unix::io::AsRawFd;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::time::{Duration, Instant};

/// Parameterversion der Spawn-Funktion (injizierbar fuer Tests).
pub struct SpawnConfig<'a> {
    pub daemon_bin: &'a Path,
    pub config: &'a Path,
    pub dump_dir: &'a Path,
    pub log_path: &'a Path,
}

/// Zustandsmaschine des Daemon-Kindprozesses.
/// Fremd-Daemon (anderer Prozess haelt `dump_dir/.lock`, W4-Flock): kein
/// Kind, nichts zu killen — Button/Tray disabled. PID ist nur Diagnose
/// (Datei kann leer sein: der Daemon truncatet vor dem PID-Write).
pub enum DaemonState {
    Stopped,
    Running { child: Child },
    Stopping { child: Child, deadline: Instant },
    Foreign { pid: Option<u32> },
}

impl DaemonState {
    pub fn is_running(&self) -> bool {
        matches!(
            self,
            DaemonState::Running { .. } | DaemonState::Stopping { .. }
        )
    }
}

/// SIGTERM senden; Kind in `Stopping` mit Deadline ueberfuehren.
pub fn stop_daemon(state: &mut DaemonState) {
    // KEIN Kind: Foreign vor mem::replace pruefen — der replace wuerde
    // den Zustand sonst nach Stopped kippen statt no-op zu bleiben.
    if matches!(state, DaemonState::Foreign { .. }) {
        return;
    }
    let child = match std::mem::replace(state, DaemonState::Stopped) {
        DaemonState::Running { child } => child,
        other => {
            *state = other;
            return;
        }
    };
    // SAFETY: child.id() ist eine echte PID unseres Kindprozesses.
    unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGTERM) };
    *state = DaemonState::Stopping {
        child,
        deadline: Instant::now() + Duration::from_secs(5),
    };
}

/// Pollt den Kindprozess (in logic()): reap bei Exit, SIGKILL nach Deadline.
/// Liefert eine Statusmeldung bei Zustandswechsel.
pub fn poll_daemon(state: &mut DaemonState) -> Option<String> {
    match state {
        DaemonState::Running { child } | DaemonState::Stopping { child, .. } => {
            if let Some(status) = child.try_wait().ok().flatten() {
                *state = DaemonState::Stopped;
                return Some(format!("Daemon beendet: {status}"));
            }
        }
        DaemonState::Stopped => return None,
        DaemonState::Foreign { .. } => return None, // kein Kind zu reapen
    }
    if let DaemonState::Stopping { child, deadline } = state
        && Instant::now() >= *deadline
    {
        let _ = child.kill(); // std kill = SIGKILL, letzte Stufe
        let _ = child.wait(); // sofort reapen
        *state = DaemonState::Stopped;
        return Some("Daemon nach Timeout mit SIGKILL beendet".into());
    }
    None
}

/// Probe-Flock auf `dir/.lock` (W4-Lock des Daemons). `None` = frei;
/// `Some(pid)` = belegt (PID aus der Datei, `None` wenn leer — der
/// Daemon truncatet die Datei, bevor er die eigene PID schreibt).
/// NUR als UX-Check vor `mount()` und im `Foreign`-Zweig — der echte
/// Konflikt-Schutz ist der Daemon-Flock selbst.
pub fn probe_daemon_lock(dir: &Path) -> Option<Option<u32>> {
    let file = match std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(dir.join(".lock"))
    {
        Ok(f) => f,
        Err(_) => return None, // kein Zugriff -> frei proben, Daemon entscheidet
    };
    // SAFETY: flock auf unserem eigenen FD, non-blocking.
    let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if ret != 0 {
        let pid = std::fs::read_to_string(dir.join(".lock"))
            .ok()
            .and_then(|s| s.trim().parse().ok());
        return Some(pid);
    }
    None // flock ging durch -> frei; Lock faellt mit dem FD-Drop
}

/// Poll-Tick fuer `Foreign`: Lock erneut proben (autoritativ — PID-Check
/// wuerde PID-Reuse und leere Lock-Datei nicht erkennen). Frei -> `Stopped`.
pub fn poll_foreign(state: &mut DaemonState, dump_dir: &Path) -> Option<String> {
    let DaemonState::Foreign { .. } = state else {
        return None;
    };
    if probe_daemon_lock(dump_dir).is_none() {
        *state = DaemonState::Stopped;
        return Some("Externer Daemon beendet — Bereit".into());
    }
    None
}

/// GUI-Exit: Kind immer mitbeenden (kein Waisen-Daemon).
/// SIGTERM, dann Spin bis Timeout, dann SIGKILL.
pub fn shutdown_daemon(state: &mut DaemonState, timeout: Duration) {
    if matches!(state, DaemonState::Foreign { .. }) {
        return; // fremder Prozess — nicht unseren Lock wegnehmen
    }
    stop_daemon(state);
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(_msg) = poll_daemon(state) {
            return;
        }
        if Instant::now() >= deadline {
            if let DaemonState::Stopping { child, .. } = state {
                let _ = child.kill();
                let _ = child.wait();
                *state = DaemonState::Stopped;
            }
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Sucht `name` in PATH (pure, testbar).
pub fn find_in_path(name: &str, path_var: Option<&str>) -> Option<PathBuf> {
    for dir in path_var.unwrap_or("").split(':') {
        if dir.is_empty() {
            continue;
        }
        let candidate = PathBuf::from(dir).join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Daemon-Binary: Sibling von current_exe (Dev: shared target-dir),
/// Fallback PATH-Suche. Sucht "crashmon" (k2: Binary-Name == Unit-Name).
pub fn find_daemon_bin() -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let sibling = dir.join("crashmon");
        if sibling.is_file() {
            return Some(sibling);
        }
    }
    find_in_path("crashmon", std::env::var("PATH").ok().as_deref())
}

/// WARUM (Landmine, nicht wegkommentieren): PDEATHSIG haengt am THREAD,
/// nicht am Prozess — stirbt der Thread, der geforkt hat, feuert es sofort.
/// Heute spawnt der Main-Thread (lebt so lange wie die App); wenn der
/// Spawn je in einen Worker-Thread wandert, muss dieser Thread fuer die
/// Lebensdauer des Kindes am Leben bleiben.
/// Effekt: kill -9 / Logout / OOM / Panic der GUI -> Kernel schickt dem
/// Daemon SIGTERM (Drain+Flush) — kein Waisen-Daemon.
/// Race-Fenster (Parent stirbt zwischen fork und prctl) schliesst der
/// getppid()-Vergleich; alle Calls sind async-signal-safe.
/// Compilierbar in dieser Form: `pre_exec` verlangt `FnMut() ->
/// io::Result<()> + Send + Sync`; getppid liefert `pid_t` (i32) -> cast.
pub fn pdeathsig_pre_exec(
    expected_ppid: u32,
) -> impl FnMut() -> io::Result<()> + Send + Sync + 'static {
    move || {
        // SAFETY: prctl/getppid/_exit sind async-signal-safe, im Kind
        // nach fork (exklusiver Kontext), kein Rust-Heap-Zugriff.
        unsafe {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            if libc::getppid() as u32 != expected_ppid {
                libc::_exit(0); // Parent starb vor prctl -> Kind ohne Sinn
            }
        }
        Ok(())
    }
}

/// Startet den Daemon: stdout/stderr in Log-Datei (NIE Pipe -> kein
/// Blockieren des GUI-Threads), stdin null.
pub fn spawn_daemon(cfg: &SpawnConfig) -> io::Result<Child> {
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(cfg.log_path)?;
    let ppid = std::process::id();
    let mut cmd = std::process::Command::new(cfg.daemon_bin);
    cmd.args([
        "--config",
        cfg.config.to_str().unwrap_or_default(),
        "--dump-dir",
        cfg.dump_dir.to_str().unwrap_or_default(),
    ])
    .stdin(std::process::Stdio::null())
    .stdout(log.try_clone()?)
    .stderr(log);
    // SAFETY: pre_exec laeuft nach fork im Kind; pdeathsig_pre_exec
    // nutzt nur async-signal-safe Calls (prctl/getppid/_exit).
    unsafe { cmd.pre_exec(pdeathsig_pre_exec(ppid)) };
    cmd.spawn()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn find_in_path_searches_order() {
        let found = find_in_path("sleep", Some("/nonexistent:/usr/bin"));
        assert_eq!(found, Some(PathBuf::from("/usr/bin/sleep")));
    }

    #[test]
    fn stop_sends_sigterm_and_reaps() {
        let child = std::process::Command::new("/usr/bin/sleep")
            .arg("30")
            .spawn()
            .expect("sleep");
        let mut state = DaemonState::Running { child };
        stop_daemon(&mut state);
        assert!(matches!(state, DaemonState::Stopping { .. }));

        // Poll bis beendet (sleep stirbt an SIGTERM sofort)
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(msg) = poll_daemon(&mut state) {
                assert!(msg.contains("beendet"), "{msg}");
                break;
            }
            assert!(Instant::now() < deadline, "SIGTERM-Exit zu langsam");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(matches!(state, DaemonState::Stopped));
    }

    #[test]
    fn stop_auf_foreign_bleibt_noop() {
        // F3-Regression: Foreign vor mem::replace pruefen, sonst kippt
        // der Zustand nach Stopped statt no-op zu bleiben.
        let mut state = DaemonState::Foreign { pid: Some(1) };
        stop_daemon(&mut state);
        assert!(
            matches!(state, DaemonState::Foreign { .. }),
            "Foreign bleibt Foreign (kein Kind zu stoppen)"
        );
    }

    #[test]
    fn sigkill_fallback_after_deadline() {
        // SIGTERM ignorierendes Kind (trap "" TERM)
        let child = std::process::Command::new("/bin/sh")
            .args(["-c", "trap '' TERM; sleep 30"])
            .spawn()
            .expect("sh");
        let mut state = DaemonState::Stopping {
            child,
            deadline: Instant::now() - Duration::from_millis(1), // Deadline abgelaufen
        };
        let msg = poll_daemon(&mut state).expect("Zustandswechsel");
        assert!(msg.contains("SIGKILL"), "{msg}");
        assert!(matches!(state, DaemonState::Stopped));
    }

    #[test]
    fn shutdown_terminates_child() {
        let child = std::process::Command::new("/usr/bin/sleep")
            .arg("30")
            .spawn()
            .expect("sleep");
        let mut state = DaemonState::Running { child };
        shutdown_daemon(&mut state, Duration::from_secs(3));
        assert!(matches!(state, DaemonState::Stopped));
    }

    #[test]
    fn spawn_writes_log_file() {
        let dir = std::env::temp_dir().join(format!("crashmon-gui-spawn-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let log_path = dir.join("log.txt");
        let cfg = SpawnConfig {
            daemon_bin: Path::new("/usr/bin/sh"),
            config: Path::new("/dev/null"),
            dump_dir: Path::new("/tmp"),
            log_path: &log_path,
        };
        let child = spawn_daemon(&cfg).expect("spawn");
        assert!(log_path.exists());
        let mut state = DaemonState::Running { child };
        shutdown_daemon(&mut state, Duration::from_secs(2));
        fs::remove_dir_all(&dir).ok();
    }

    /// Haelt den Test-Flock in einem separaten Prozess (kein Fork-Hazard
    /// durch parallele Test-Kinder) und raeumt IMMER auf: Drop killt die
    /// ganze Prozessgruppe (flock fork't das Kommando) — auch bei Panic
    /// im Test (Rot-Pfad: sonst 30-s-Orphan mit flocktem FD + Pipe).
    struct FlockHolder(std::process::Child);

    impl FlockHolder {
        fn acquire(dir: &std::path::Path) -> Self {
            let _ = fs::File::create(dir.join(".lock")); // Absicherung gegen flock-Versionen ohne O_CREAT
            let holder = std::process::Command::new("flock")
                .arg(dir.join(".lock"))
                .args(["-c", "sleep 30"])
                .process_group(0) // eigene Gruppe -> Kill(-pid) trifft auch das geforkte Kind
                .spawn()
                .expect("flock-util (util-linux)");
            let pid = holder.id();
            let deadline = Instant::now() + Duration::from_secs(2);
            while probe_daemon_lock(dir).is_none() {
                assert!(
                    Instant::now() < deadline,
                    "flock-util hat den Lock nie genommen"
                );
                std::thread::sleep(Duration::from_millis(20));
            }
            let _ = pid;
            Self(holder)
        }
    }

    impl Drop for FlockHolder {
        fn drop(&mut self) {
            let pid = self.0.id();
            // SAFETY: kill auf unsere eigene frisch erzeugte Prozessgruppe
            // (process_group(0)) — kein Kollateral.
            unsafe { libc::kill(-(pid as libc::pid_t), libc::SIGKILL) };
            let _ = self.0.wait();
        }
    }

    #[test]
    fn probe_lock_frei_belegt_und_leere_datei() {
        let dir = std::env::temp_dir().join(format!("crashmon-gui-flock-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        assert_eq!(probe_daemon_lock(&dir), None, "kein Lock -> frei");

        // WARUM (Flake-Fix, nicht vereinfachen): Lock NICHT im Test-Thread
        // flocken — parallele Test-Kinder (sleep 30 aus Spawn-Tests) erben
        // beim fork->exec den flockten Test-FD, halten den Lock kurz und der
        // Test-close gibt ihn dann nicht frei -> EWOULDBLOCK-Falsch-rot. Deshalb
        // haelt ein SEPARATER flock-Prozess den Lock: kein fremder Test-Prozess
        // erbt den FD, das Problem existiert nicht mehr.
        let _holder = FlockHolder::acquire(&dir);
        fs::write(dir.join(".lock"), std::process::id().to_string()).unwrap();
        assert_eq!(
            probe_daemon_lock(&dir),
            Some(Some(std::process::id())),
            "belegt mit PID"
        );
        fs::write(dir.join(".lock"), "").unwrap(); // leer (set_len(0)-Fenster)
        assert_eq!(probe_daemon_lock(&dir), Some(None), "belegt, PID leer");
        // _holder-Drop killt die flock-Prozessgruppe — auch bei Panic im
        // Test (kein 30-s-Orphan mit flocktem FD + Pipe).
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn foreign_wird_bei_freiem_lock_zu_stopped() {
        let dir = std::env::temp_dir().join(format!("crashmon-gui-frel-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut state = DaemonState::Foreign { pid: Some(999999) };
        let msg = poll_foreign(&mut state, &dir).expect("Zustandswechsel");
        assert!(msg.contains("Externer Daemon"), "{msg}");
        assert!(matches!(state, DaemonState::Stopped));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn foreign_bleibt_wenn_lock_belegt() {
        let dir = std::env::temp_dir().join(format!("crashmon-gui-flbl-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let file = fs::File::create(dir.join(".lock")).unwrap();
        unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        let mut state = DaemonState::Foreign { pid: Some(123) };
        assert_eq!(poll_foreign(&mut state, &dir), None, "Lock noch belegt");
        assert!(matches!(state, DaemonState::Foreign { .. }));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn foreign_is_not_running() {
        let state = DaemonState::Foreign { pid: Some(1) };
        assert!(!state.is_running());
    }
}
