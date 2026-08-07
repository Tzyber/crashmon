//! CrashmonGui: eframe::App — Panels, Mount/Stop, Report-Liste, Detail, Log.
//!
//! eframe 0.36-API: `logic()` (Poll, laeuft auch bei verstecktem Fenster)
//! + `ui()` (Panels nehmen das Root-Ui). Kein tokio, ein Thread — alle
//!   Poll-Operationen sind sub-Millisekunden.

use crate::config::{ensure_default_config, state_dir};
use crate::fetch::{self, Answer};
use crate::format::{format_ts, summarize};
use crate::logtail::LogTail;
use crate::reference::{knowledge_default, reference_lines, xid_info};
use crate::scan::scan_dir;
use crate::state::{
    DaemonState, SpawnConfig, poll_daemon, shutdown_daemon, spawn_daemon, stop_daemon,
};
use crash_daemon::event::{CrashEvent, EventKind};
use crash_daemon::output::Report;
use std::collections::{BTreeMap, HashSet};
use std::io;
use std::path::PathBuf;
use std::process::Child;
use std::time::Duration;

/// Log-Tail-Puffergroesse (Zeilen).
const LOG_MAX_LINES: usize = 500;

/// Injizierbare Spawn-Funktion (Tests nutzen einen Fake).
type SpawnFn = Box<dyn Fn(&SpawnConfig) -> io::Result<Child>>;

/// Ergebnis eines Auto-Lookups (Hintergrund-Thread -> GUI-Thread).
type LearnResult = (u16, Result<Option<Answer>, String>);

pub struct CrashmonGui {
    state_dir: PathBuf,
    dump_dir: PathBuf,
    daemon: DaemonState,
    reports: BTreeMap<u64, Report>, // ts -> Report; Rendering absteigend
    selected: Option<u64>,
    newest_ts: Option<u64>, // NEU-Badge (zuletzt hinzugefuegt)
    corrupt: u64,
    log: LogTail,
    status: String,
    /// Wissensspeicher (knowledge.md im state_dir) — User-editierbar.
    knowledge: String,
    /// Auto-Lookup: bereits nachgeschlagene Xid-Codes (kein Netzwerk-Spam).
    looked_up: HashSet<u16>,
    /// Ergebnis eines laufenden Lookups (Thread -> GUI-Thread).
    learn_rx: Option<std::sync::mpsc::Receiver<LearnResult>>,
    /// Injizierbar fuer Tests (Fake-Spawner).
    spawn: SpawnFn,
}

impl CrashmonGui {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let state_dir = state_dir();
        let dump_dir = state_dir.clone();
        Self {
            state_dir,
            dump_dir,
            daemon: DaemonState::Stopped,
            reports: BTreeMap::new(),
            selected: None,
            newest_ts: None,
            corrupt: 0,
            log: LogTail::new(LOG_MAX_LINES),
            status: "Bereit — Daemon starten".into(),
            knowledge: knowledge_default(),
            looked_up: HashSet::new(),
            learn_rx: None,
            spawn: Box::new(spawn_daemon),
        }
    }

    /// Laedt den Wissensspeicher: Vorlage bei Erststart, danach laufend
    /// MERGE — fehlende Vorlagen-Sektionen werden angehaengt, eigene
    /// Eintraege und Auto-gelerntes bleiben unangetastet (kein Loeschen
    /// noetig, wenn die Vorlage waechst).
    fn refresh_knowledge(&mut self) {
        let path = self.state_dir.join("knowledge.md");
        if !path.exists()
            && let Err(e) = std::fs::write(&path, knowledge_default())
        {
            self.status = format!("Wissensspeicher-Anlage fehlgeschlagen: {e}");
            return;
        }
        match std::fs::read_to_string(&path) {
            Ok(content) => {
                match crate::reference::merge_knowledge(&content, &knowledge_default()) {
                    Some(merged) => {
                        if let Err(e) = std::fs::write(&path, &merged) {
                            self.status = format!("Wissensspeicher-Merge fehlgeschlagen: {e}");
                            return;
                        }
                        self.status = "Wissensspeicher um neue Sektionen erweitert".into();
                        self.knowledge = merged;
                    }
                    None => self.knowledge = content,
                }
            }
            Err(e) => self.status = format!("Wissensspeicher unlesbar: {e}"),
        }
    }

    /// Auto-Lookup fuer einen unbekannten Xid-Code: einmalig pro Code,
    /// Abruf im Hintergrund-Thread (GUI bleibt fluessig bei 10-s-Timeout).
    fn lookup_xid(&mut self, code: u16) {
        if !self.looked_up.insert(code) {
            return; // laeuft bereits / war schon nachgeschlagen
        }
        self.status = format!("Suche Infos zu Xid {code} (DuckDuckGo)...");
        let (tx, rx) = std::sync::mpsc::channel();
        self.learn_rx = Some(rx);
        std::thread::spawn(move || {
            let result = fetch::fetch_answer(&format!("NVRM Xid {code}"));
            let _ = tx.send((code, result));
        });
    }

    /// Uebernimmt ein abgeschlossenes Lookup-Ergebnis in knowledge.md
    /// (Auto-Learning: die Wissensdatenbank waechst von selbst).
    fn apply_learning(&mut self) {
        let Some(rx) = &self.learn_rx else { return };
        let Ok((code, result)) = rx.try_recv() else {
            return;
        };
        self.learn_rx = None;
        match result {
            Ok(Some(Answer { text, url })) => {
                let entry =
                    format!("\n## Auto-gelernt (DDG): Xid {code}\n- {text}\n- Quelle: {url}\n");
                let path = self.state_dir.join("knowledge.md");
                match std::fs::OpenOptions::new().append(true).open(&path) {
                    Ok(mut f) => {
                        use std::io::Write;
                        if let Err(e) = f.write_all(entry.as_bytes()) {
                            self.status = format!("Wissensspeicher-Anhang fehlgeschlagen: {e}");
                            return;
                        }
                        self.status = format!("Xid {code} gelernt (Quelle: {url})");
                    }
                    Err(e) => {
                        self.status = format!("Wissensspeicher unlesbar: {e}");
                        return;
                    }
                }
                self.refresh_knowledge();
            }
            Ok(None) => {
                self.status = format!(
                    "Xid {code}: keine Treffer bei DuckDuckGo — selbst in knowledge.md ergänzen"
                );
            }
            Err(e) => self.status = format!("Xid {code}: {e}"),
        }
    }

    /// Test-Konstruktor mit injiziertem Spawner.
    #[cfg(test)]
    fn with_spawner(
        state_dir: PathBuf,
        spawn: Box<dyn Fn(&SpawnConfig) -> io::Result<Child>>,
    ) -> Self {
        let dump_dir = state_dir.clone();
        Self {
            state_dir,
            dump_dir,
            daemon: DaemonState::Stopped,
            reports: BTreeMap::new(),
            selected: None,
            newest_ts: None,
            corrupt: 0,
            log: LogTail::new(LOG_MAX_LINES),
            status: "Bereit".into(),
            knowledge: knowledge_default(),
            looked_up: HashSet::new(),
            learn_rx: None,
            spawn,
        }
    }

    fn mount(&mut self) {
        let config = match ensure_default_config(&self.state_dir) {
            Ok(c) => c,
            Err(e) => {
                self.status = format!("Config-Erzeugung fehlgeschlagen: {e}");
                return;
            }
        };
        let Some(bin) = crate::state::find_daemon_bin() else {
            self.status = "crash-daemon Binary nicht gefunden".into();
            return;
        };
        let cfg = SpawnConfig {
            daemon_bin: &bin,
            config: &config,
            dump_dir: &self.dump_dir,
            log_path: &self.state_dir.join("crashmon-daemon.log"),
        };
        match (self.spawn)(&cfg) {
            Ok(child) => {
                self.daemon = DaemonState::Running { child };
                self.status = format!("Daemon läuft (PID {})", self.daemon_pid().unwrap_or(0));
            }
            Err(e) => self.status = format!("Start fehlgeschlagen: {e}"),
        }
    }

    fn stop(&mut self) {
        stop_daemon(&mut self.daemon);
        self.status = "Daemon wird gestoppt (SIGTERM, Drain + Flush)...".into();
    }

    fn daemon_pid(&self) -> Option<u32> {
        match &self.daemon {
            DaemonState::Running { child } | DaemonState::Stopping { child, .. } => {
                Some(child.id())
            }
            DaemonState::Stopped => None,
        }
    }

    /// Poll: Kindprozess, neue Reports, Log-Tail.
    fn poll(&mut self, ctx: &egui::Context) {
        if let Some(msg) = poll_daemon(&mut self.daemon) {
            self.status = msg;
            ctx.request_repaint();
        }
        self.scan(ctx);
        self.log
            .refresh(&self.state_dir.join("crashmon-daemon.log"));
        self.apply_learning();
        self.refresh_knowledge();
    }

    /// Neue Report-Dateien einsammeln (nur neue ts; Auto-Select-Regel).
    fn scan(&mut self, ctx: &egui::Context) {
        let out = scan_dir(&self.dump_dir);
        if out.corrupt > 0 {
            self.corrupt = self.corrupt.saturating_add(out.corrupt);
            self.status = format!("{} unlesbare Dateien übersprungen", self.corrupt);
        }
        let mut changed = false;
        for (ts, report) in out.reports {
            if self.reports.contains_key(&ts) {
                continue;
            }
            // Auto-Learning: unbekannter Xid-Code -> automatisch nachschlagen
            if let EventKind::GpuXid { code, .. } = &report.cause.kind
                && xid_info(*code).0 == "unbekannt"
            {
                self.lookup_xid(*code);
            }
            self.reports.insert(ts, report);
            self.newest_ts = Some(ts);
            changed = true;
        }
        // Auto-Select nur wenn der User noch nichts gewaehlt hat — dann den
        // NEUESTEN Report (User-Auswahl wird nie ueberschrieben).
        if changed && self.selected.is_none() {
            self.selected = self.newest_ts;
        }
        if changed {
            self.status = format!(
                "Report empfangen: {}",
                format_ts(self.newest_ts.unwrap_or(0))
            );
            ctx.request_repaint();
        }
    }

    // --- UI-Teilmethoden (getrennt fuer Kittest) ---------------------------

    fn header_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("crashmon");
            let (label, enabled) = match self.daemon {
                DaemonState::Stopped => ("▶ Daemon starten", true),
                DaemonState::Running { .. } => ("■ Daemon stoppen", true),
                DaemonState::Stopping { .. } => ("⏳ stoppt...", false),
            };
            if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
                if self.daemon.is_running() {
                    self.stop();
                } else {
                    self.mount();
                }
            }
            ui.separator();
            ui.label(&self.status);
        });
    }

    fn list_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Reports");
        ui.separator();
        egui::ScrollArea::vertical().show(ui, |ui| {
            let reports: Vec<(u64, &Report)> =
                self.reports.iter().rev().map(|(t, r)| (*t, r)).collect();
            for (ts, report) in reports {
                let is_new = self.newest_ts == Some(ts);
                let label = if is_new {
                    format!("🆕 {} — {}", format_ts(ts), summarize(report))
                } else {
                    format!("{} — {}", format_ts(ts), summarize(report))
                };
                let selected = self.selected == Some(ts);
                if ui.selectable_label(selected, label).clicked() {
                    self.selected = Some(ts);
                }
            }
            if self.reports.is_empty() {
                ui.weak("Noch keine Reports — starte den Daemon und erzeuge einen Crash.");
            }
        });
    }

    fn detail_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Detail");
        ui.separator();
        let Some(ts) = self.selected else {
            ui.weak("Report in der Liste auswählen.");
            return;
        };
        let Some(report) = self.reports.get(&ts) else {
            return;
        };
        ui.horizontal(|ui| {
            ui.monospace(format!("Zeit: {}", format_ts(report.ts)));
            if ui.button("📋 JSON kopieren").clicked() {
                let json = serde_json::to_string_pretty(report).unwrap_or_default();
                ui.output_mut(|o| {
                    o.commands.push(egui::output::OutputCommand::CopyText(json));
                });
                self.status = "Report-JSON kopiert".into();
            }
        });
        ui.monospace(format!("Lost-Events: {}", report.lost_events));
        ui.separator();
        ui.strong("Ursache");
        render_event(ui, &report.cause);
        if !report.related.is_empty() {
            ui.separator();
            ui.strong(format!("Beigeordnet ({})", report.related.len()));
            for ev in &report.related {
                render_event(ui, ev);
                ui.separator();
            }
        }

        // Eingebaute Wissensbasis (Xid-Severity, Event-Erklaerungen)
        ui.separator();
        ui.strong("Referenz");
        for (title, text) in reference_lines(&report.cause.kind) {
            ui.monospace(format!("{title}: {text}"));
        }
        // Manuelles Nachschlagen (Auto-Lookup laeuft bei unbekannten Xids)
        if let EventKind::GpuXid { code, .. } = &report.cause.kind
            && ui.button(format!("🔍 Xid {code} nachschlagen")).clicked()
        {
            self.lookup_xid(*code);
        }

        // Lokaler Wissensspeicher (knowledge.md, User-editierbar)
        ui.separator();
        egui::CollapsingHeader::new("Wissensspeicher (knowledge.md)")
            .default_open(false)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(180.0)
                    .show(ui, |ui| {
                        for line in self.knowledge.lines() {
                            ui.monospace(line);
                        }
                    });
                ui.weak(format!(
                    "Editierbar: {}",
                    self.state_dir.join("knowledge.md").display()
                ));
            });
    }

    fn log_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Daemon-Log");
            ui.weak(format!("({} Zeilen Tail)", self.log.lines().count()));
        });
        ui.separator();
        egui::ScrollArea::vertical()
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for line in self.log.lines() {
                    ui.monospace(line);
                }
            });
    }
}

/// Formatiertes Event-Detail (statt rohem JSON).
fn render_event(ui: &mut egui::Ui, ev: &CrashEvent) {
    ui.monospace(format!("Zeit: {}", format_ts(ev.ts)));
    match &ev.kind {
        EventKind::Coredump {
            pid,
            exe,
            comm,
            signal,
            uid,
            unit,
            coredump_file,
        } => {
            ui.monospace("Art:   Coredump");
            ui.monospace(format!("PID:   {pid}"));
            if let Some(exe) = exe {
                ui.monospace(format!("EXE:   {exe}"));
            }
            ui.monospace(format!("COMM:  {comm}"));
            if let Some(s) = signal {
                ui.monospace(format!("SIGNAL: {s}"));
            }
            if let Some(u) = uid {
                ui.monospace(format!("UID:   {u}"));
            }
            if let Some(u) = unit {
                ui.monospace(format!("UNIT:  {u}"));
            }
            if let Some(f) = coredump_file {
                ui.monospace(format!("CORE:  {f}"));
            }
        }
        EventKind::OomKill { pid, comm } => {
            ui.monospace("Art:   OOM-Kill");
            ui.monospace(format!("PID:   {pid}"));
            ui.monospace(format!("COMM:  {comm}"));
        }
        EventKind::GpuXid {
            code,
            pci,
            pid,
            message,
        } => {
            ui.monospace(format!("Art:   NVIDIA Xid {code}"));
            if let Some(p) = pci {
                ui.monospace(format!("PCI:   {p}"));
            }
            if let Some(p) = pid {
                ui.monospace(format!("PID:   {p}"));
            }
            ui.monospace(format!("MSG:   {message}"));
        }
        EventKind::GpuReset { vendor, detail } => {
            ui.monospace(format!("Art:   GPU Reset ({vendor})"));
            ui.monospace(format!("MSG:   {detail}"));
        }
        EventKind::GpuWedged { method } => {
            ui.monospace("Art:   GPU Wedged");
            if let Some(m) = method {
                ui.monospace(format!("METH:  {m}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crash_daemon::event::EventKind;
    use std::fs;

    /// Fake-Spawner: erzeugt einen echten sleep-Child, ignoriert cfg.
    fn fake_spawner(_cfg: &SpawnConfig) -> io::Result<Child> {
        std::process::Command::new("/usr/bin/sleep")
            .arg("30")
            .spawn()
    }

    fn temp_state(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("crashmon-gui-app-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn write_report(dir: &std::path::Path, ts: u64) {
        let report = Report {
            ts,
            cause: CrashEvent {
                ts,
                kind: EventKind::GpuWedged {
                    method: Some("rebind".into()),
                },
            },
            related: vec![],
            lost_events: 0,
        };
        fs::write(
            dir.join(format!("crash-{ts}.json")),
            serde_json::to_string(&report).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn scan_adds_new_reports_and_autoselects() {
        let dir = temp_state("scan");
        write_report(&dir, 1_000_000);
        write_report(&dir, 2_000_000);
        let mut app = CrashmonGui::with_spawner(dir.clone(), Box::new(fake_spawner));
        app.dump_dir = dir.clone();

        let ctx = egui::Context::default();
        app.scan(&ctx);

        assert_eq!(app.reports.len(), 2);
        assert_eq!(app.newest_ts, Some(2_000_000), "neueste markiert");
        assert_eq!(
            app.selected,
            Some(2_000_000),
            "Auto-Select (vorher nichts gewaehlt)"
        );
        assert!(app.status.contains("Report empfangen"), "{}", app.status);

        // Idempotent: zweiter Scan fuegt nichts hinzu
        app.scan(&ctx);
        assert_eq!(app.reports.len(), 2);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn scan_counts_corrupt_files() {
        let dir = temp_state("corrupt");
        write_report(&dir, 1_000_000);
        fs::write(dir.join("crash-9999999999999.json"), "garbage").unwrap();
        let mut app = CrashmonGui::with_spawner(dir.clone(), Box::new(fake_spawner));
        app.dump_dir = dir.clone();

        app.scan(&egui::Context::default());
        assert_eq!(app.reports.len(), 1, "gueltiger bleibt");
        assert_eq!(
            app.corrupt, 1,
            "Zaehler erhoeht (Status-Text ist transiente Info)"
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn kittest_click_mount_button() {
        // Headless-UI-Smoke (egui_kittest, kein Display noetig):
        // Klick auf "Daemon starten" -> Zustand Running + Label wechselt.
        let dir = temp_state("kittest");
        // PATH-Fake wie im mount-Test (find_daemon_bin)
        let fake_bin = dir.join("crash-daemon");
        fs::write(&fake_bin, "#!/bin/sh\nexec /usr/bin/sleep 30\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&fake_bin, fs::Permissions::from_mode(0o755)).unwrap();
        let old_path = std::env::var("PATH").ok();
        unsafe { std::env::set_var("PATH", dir.to_str().unwrap()) };

        let app = CrashmonGui::with_spawner(dir.clone(), Box::new(fake_spawner));
        let mut harness =
            egui_kittest::Harness::new_ui_state(|ui, app: &mut CrashmonGui| app.header_ui(ui), app);

        // Klick auf den Start-Button (AccessKit-Label; Queryable-Trait in Scope)
        use egui_kittest::kittest::Queryable;
        harness.get_by_label("▶ Daemon starten").click();
        harness.run();
        assert!(
            harness.state().daemon.is_running(),
            "Klick muss Daemon starten (status: {})",
            harness.state().status
        );

        // Aufraeumen
        let mut app = harness.into_state();
        shutdown_daemon(&mut app.daemon, Duration::from_secs(3));
        match old_path {
            Some(p) => unsafe { std::env::set_var("PATH", p) },
            None => unsafe { std::env::remove_var("PATH") },
        }
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn mount_sets_running_with_fake_spawner() {
        let dir = temp_state("mount");
        // PATH auf Fake-crash-daemon-Skript zeigen lassen (find_daemon_bin)
        let fake_bin = dir.join("crash-daemon");
        fs::write(&fake_bin, "#!/bin/sh\nexec /usr/bin/sleep 30\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&fake_bin, fs::Permissions::from_mode(0o755)).unwrap();
        // Achtung: set_var ist prozess-global — alten PATH sichern/zuruecksetzen
        let old_path = std::env::var("PATH").ok();
        unsafe { std::env::set_var("PATH", dir.to_str().unwrap()) };

        let mut app = CrashmonGui::with_spawner(dir.clone(), Box::new(fake_spawner));
        app.mount();
        assert!(app.daemon.is_running(), "status: {}", app.status);
        assert!(app.status.contains("PID"), "status: {}", app.status);

        // Aufraeumen: Kind beenden + PATH zurueck
        shutdown_daemon(&mut app.daemon, Duration::from_secs(3));
        match old_path {
            Some(p) => unsafe { std::env::set_var("PATH", p) },
            None => unsafe { std::env::remove_var("PATH") },
        }
        fs::remove_dir_all(&dir).ok();
    }
}

impl eframe::App for CrashmonGui {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(Duration::from_secs(1));
        self.poll(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // egui 0.36: einheitliches Panel-API (TopBottomPanel/SidePanel
        // entfallen), show nimmt das Root-Ui.
        egui::Panel::top("header").show(ui, |ui| self.header_ui(ui));
        egui::Panel::bottom("log")
            .resizable(true)
            .default_size(140.0)
            .show(ui, |ui| self.log_ui(ui));
        egui::Panel::left("reports")
            .default_size(340.0)
            .show(ui, |ui| self.list_ui(ui));
        egui::CentralPanel::default().show(ui, |ui| self.detail_ui(ui));
    }

    fn on_exit(&mut self) {
        // Fenster zu -> Daemon immer mitbeenden (kein Waisen-Prozess).
        // Blocking ist hier ok (Fenster bereits geschlossen).
        shutdown_daemon(&mut self.daemon, Duration::from_secs(3));
    }
}
