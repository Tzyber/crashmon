//! crashmon-gui — Desktop-Oberflaeche fuer den crashmon-Crash-Daemon.
//!
//! Startet/stoppt den Daemon als Kindprozess (kein Root) und zeigt die
//! JSON-Reports formatiert an (Live-Update). eframe/egui 0.36-API:
//! `App::ui(&mut Ui)` + `App::logic(&mut ctx)` (0.34-Breaking-Change).

mod app;
mod config;
mod format;
mod logtail;
mod reference;
mod scan;
mod state;

use app::CrashmonGui;

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("crashmon — Crash-Daemon")
            .with_inner_size([980.0, 640.0])
            .with_min_inner_size([700.0, 420.0]),
        centered: true,
        ..Default::default()
    };
    eframe::run_native(
        "crashmon-gui", // Wayland-App-ID
        options,
        Box::new(|cc| Ok(Box::new(CrashmonGui::new(cc)))),
    )
}
