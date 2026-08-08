//! crashmon-gui — Desktop-Oberflaeche fuer den crashmon-Crash-Daemon.
//!
//! Nur Startpunkt: eframe-API + App-Konstruktion. Alle Module leben in
//! der Lib (crashmon_gui), damit Test-Bins (pdeath_helper) sie sehen.

use crashmon_gui::app::CrashmonGui;

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
