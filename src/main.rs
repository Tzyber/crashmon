//! Linux Crash-Daemon — Entry Point (duenne Binary ueber `crash_daemon`-Lib).
//!
//! Leichtgewichtiger, unprivilegierter Daemon: erfasst Prozessabstuerze
//! (systemd-coredump), OOM-Killer-Events und GPU-Hangs (amdgpu/NVIDIA/Intel)
//! aus dem systemd-Journal + Kernel-Uevents und schreibt strukturierte
//! JSON-Reports. Siehe `openspec/specs/` fuer Capability-Spezifikationen.

use clap::Parser;
use crash_daemon::{config, daemon};
use std::io::IsTerminal;
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(name = "crashmon", version, about = "Lightweight Linux crash daemon")]
struct Cli {
    /// Pfad zur TOML-Konfiguration.
    #[arg(long, default_value = "/etc/crashmon/config.toml")]
    config: PathBuf,

    /// Ausgabe-Verzeichnis fuer JSON-Reports (ueberschreibt Config).
    #[arg(long)]
    dump_dir: Option<PathBuf>,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let mut cfg = config::Config::load(&cli.config)?;
    if let Some(dir) = cli.dump_dir {
        cfg.dump_dir = dir;
    }

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new(&cfg.log_level))
        // ANSI nur, wenn stdout wirklich ein Terminal ist. tracing-subscriber
        // faerbt sonst auch dann, wenn die GUI stdout in eine Datei umlenkt —
        // die Escape-Sequenzen stehen dann als Muell im Log.
        .with_ansi(std::io::stdout().is_terminal())
        .init();

    tracing::info!("crashmon starting, dump_dir={}", cfg.dump_dir.display());

    daemon::run(cfg).await
}
