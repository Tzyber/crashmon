//! Test-Helper fuer den PDEATHSIG-E2E: spawnt ein Kind mit demselben
//! pre_exec-Hook wie spawn_daemon, schreibt die Kind-PID auf stdout
//! und killt sich dann selbst mit SIGKILL. Der Test prueft, ob das Kind
//! stirbt (Kernel -> SIGTERM via PDEATHSIG).
//! Reihenfolge zaehlt: spawn -> PID schreiben -> FLUSH -> SIGKILL
//! (stdout an eine Pipe ist block-buffered; SIGKILL flusht nichts).

use crashmon_gui::state::pdeathsig_pre_exec;
use std::io::Write;
use std::os::unix::process::CommandExt;

fn main() {
    let ppid = std::process::id();
    let mut cmd = std::process::Command::new("/usr/bin/sleep");
    cmd.arg("120");
    // SAFETY: pre_exec im Kind, gleicher Hook wie spawn_daemon.
    unsafe { cmd.pre_exec(pdeathsig_pre_exec(ppid)) };
    let child = cmd.spawn().expect("sleep spawnen");
    let pid = child.id();
    let mut out = std::io::stdout().lock();
    writeln!(out, "{pid}").expect("PID schreiben");
    out.flush().expect("PID flush vor SIGKILL");
    // SIGKILL auf uns selbst: on_exit/atexit laufen NICHT — genau der
    // zu testende Pfad (Parent verschwindet ungeordnet).
    unsafe { libc::kill(std::process::id() as libc::pid_t, libc::SIGKILL) };
    std::process::exit(1); // nie erreicht
}
