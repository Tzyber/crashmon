//! CrashmonGui: eframe::App — Header, Report-Liste (Filter), Detail, Log.
//!
//! eframe 0.36-API: `logic()` (Poll, laeuft auch bei verstecktem Fenster)
//! + `ui()` (Panels nehmen das Root-Ui). Kein tokio, ein Thread.
//!
//! Review-Umbau (crashmon-review.md rev. 2):
//! - W1: Scan/Log nicht mehr pro Frame — 500-ms-Debounce, bekannte ts
//!   werden gar nicht mehr geoeffnet (scan_dir nimmt known-Set)
//! - W2: corrupt = Gesamtzahl pro Scan (kein saturating_add der Summe)
//! - k1/Auto-Learning: kein DuckDuckGo-API-Abruf mehr — „im Browser
//!   suchen" (xdg-open); kein Netzwerkcode, keine learn_rx-Bugs
//! - D1..D6: Grid-Layout, Severity-Farben, zweizeilige Liste, Textlabels,
//!   Log aufklappbar, Status-Leiste, Wissensspeicher-Fenster, Filter,
//!   Fenstergroesse-Persistenz, Empty-State, Report loeschen
//! - k8: daemon_bin injizierbar (kein prozessglobales PATH-set_var)

use crate::config::{ensure_default_config, state_dir};
use crate::format::{format_ts_local, relative_ago, summarize};
use crate::logtail::LogTail;
use crate::reference::{knowledge_default, reference_lines};
use crate::scan::scan_dir;
use crate::state::{
    DaemonState, SpawnConfig, find_daemon_bin, poll_daemon, poll_foreign, probe_daemon_lock,
    shutdown_daemon, spawn_daemon, stop_daemon,
};
use crate::tray::{CrashmonTray, TrayCmd};
use crash_daemon::event::{CrashEvent, EventKind};
use crash_daemon::gpu::matcher::{Severity, event_severity};
use crash_daemon::output::Report;
use std::collections::{BTreeMap, HashSet};
use std::io;
use std::path::PathBuf;
use std::process::Child;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Log-Tail-Puffergroesse (Zeilen).
const LOG_MAX_LINES: usize = 500;
/// D6: Key fuer die persistierte Fenstergroesse (egui-Memory).
const WINDOW_SIZE_KEY: &str = "window_size";
/// W1: Poll-Debounce — repaint passiert bei Mausbewegung mit 60 fps, das
/// Report-Verzeichnis wird hoechstens alle 500 ms gelesen.
const POLL_INTERVAL: Duration = Duration::from_millis(500);
/// Auswahlmarke in der Report-Liste: Balkenbreite + Hintergrund-Alpha
/// (0..=255). Bewusst niedrig — die Auswahl soll erkennbar sein, nicht
/// dominant. Wer sie kraeftiger will, dreht hier.
const SEL_BAR_WIDTH: f32 = 3.0;
const SEL_BG_ALPHA: u8 = 14;

/// Injizierbare Spawn-Funktion (Tests nutzen einen Fake).
type SpawnFn = Box<dyn Fn(&SpawnConfig) -> io::Result<Child>>;

/// X-Klick-Entscheidung (pure, Review-Tabelle): ohne Tray beendet X die
/// App (sonst waere sie unerreichbar); mit Tray versteckt X; waehrend
/// quitting (Tray-Menue "Beenden") laeuft X weiter zum echten Exit —
/// sonst kaeme man ueber das Tray nie raus.
#[derive(PartialEq, Debug)]
pub enum CloseAction {
    Hide,
    Proceed,
}

pub fn close_action(tray_active: bool, quitting: bool) -> CloseAction {
    if tray_active && !quitting {
        CloseAction::Hide
    } else {
        CloseAction::Proceed
    }
}

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
    /// k8: injizierbarer Daemon-Pfad (Tests); None = Auto-Detect.
    daemon_bin: Option<PathBuf>,
    /// Injizierbar fuer Tests (Fake-Spawner).
    spawn: SpawnFn,
    /// W1: Zeitpunkt des letzten Verzeichnis-/Log-Scans.
    last_poll: Instant,
    /// W1: bekannte Report-ts — scan_dir oeffnet sie nicht mehr.
    known_ts: HashSet<u64>,
    /// D6: Filtertext fuer die Report-Liste.
    filter: String,
    /// D5: Log-Panel aufklappbar (Default zu).
    show_log: bool,
    /// D5: Wissensspeicher in eigenem Fenster (statt Detailspalte).
    show_knowledge: bool,
    /// Task 17: notify-send bei neuem Report (stiller Fehlschlag ohne Tool).
    notify: bool,
    /// D6: Fenstergroesse merken (Persistenz via egui-Context-Daten —
    /// eframe persistiert die bei Exit automatisch).
    window_size: Option<egui::Vec2>,
    /// Tray-Modus aktiv (Spawn erfolgreich). false -> X beendet die App.
    tray_active: bool,
    /// Tray-Telegramme (D-Bus-Thread -> GUI-Thread).
    tray_rx: std::sync::mpsc::Receiver<TrayCmd>,
    /// ksni-Handle fuer handle.update() an Zustandsflanken (D-Bus-Roundtrip!).
    /// `blocking::Handle` — das root-`ksni::Handle` ist die async-Variante.
    tray_handle: Option<ksni::blocking::Handle<CrashmonTray>>,
    /// Fenster gerade versteckt (Tray-Betrieb).
    hidden: bool,
    /// Tray "Beenden" empfangen -> naechster Close wird echter Exit.
    quitting: bool,
    /// scan() setzt das Flag; logic() macht den update()-Roundtrip daraus.
    tray_dirty: bool,
    /// TOCTOU-Nachlauf: mount() setzt es nach erfolgreichem EIGENEM Spawn;
    /// logic() konsumiert es genau einmal, wenn der Daemon sofort stirbt —
    /// dann prueft sie den W4-Flock auf den Sieger (Foreign) statt der
    /// generischen "exit status"-Meldung.
    just_spawned: bool,
}

impl CrashmonGui {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let state_dir = state_dir();
        let dump_dir = state_dir.clone();

        // D6: persistierte Fenstergroesse wiederherstellen. egui-Memory
        // (persisted data) wird von eframe bei Exit automatisch gespeichert.
        let window_size: Option<egui::Vec2> = cc
            .egui_ctx
            .data_mut(|d| d.get_persisted(egui::Id::new(WINDOW_SIZE_KEY)));
        if let Some(size) = window_size {
            cc.egui_ctx
                .send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
        }

        let (tray_tx, tray_rx) = std::sync::mpsc::channel();
        // `blocking::TrayMethods` — das root-`TrayMethods::spawn` ist async
        // (Future, kein Result -> wuerde nicht kompilieren).
        let (tray_active, tray_handle) =
            match ksni::blocking::TrayMethods::spawn(CrashmonTray::new(tray_tx)) {
                Ok(handle) => (true, Some(handle)),
                Err(e) => {
                    eprintln!(
                        "crashmon-gui: Tray nicht verfuegbar ({e}) — Fenster-X beendet die App"
                    );
                    (false, None)
                }
            };

        let mut gui = Self {
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
            daemon_bin: None,
            spawn: Box::new(spawn_daemon),
            last_poll: Instant::now(),
            known_ts: HashSet::new(),
            filter: String::new(),
            show_log: false,
            show_knowledge: false,
            notify: true,
            window_size: None,
            tray_active,
            tray_rx,
            tray_handle,
            hidden: false,
            quitting: false,
            tray_dirty: false,
            just_spawned: false,
        };
        // W1: Wissensspeicher EINMAL beim Start laden (nicht im Poll-Pfad).
        gui.refresh_knowledge();
        gui.mount(); // Autostart: Tray-Betrieb ohne laufenden Daemon ist sinnlos
        gui
    }

    /// Laedt den Wissensspeicher: Vorlage bei Erststart, danach laufend
    /// MERGE — fehlende Vorlagen-Sektionen werden angehaengt, eigene
    /// Eintraege bleiben unangetastet (kein Loeschen noetig, wenn die
    /// Vorlage waechst). Nur auf Anforderung (Start, „Neu laden") —
    /// W1: nicht im Poll-Pfad.
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

    /// Auto-Learning-Umbau (Review): statt DuckDuckGo-API-Abruf oeffnet der
    /// Knopf die vorformulierte Suche im Standard-Browser (xdg-open).
    /// Null Netzwerkcode, null Datenschutzfrage — der User entscheidet,
    /// was er in die Wissensdatei uebernimmt.
    fn open_browser_search(&mut self, query: &str) {
        let url = format!("https://duckduckgo.com/?q={}", urlencode(query));
        match std::process::Command::new("xdg-open").arg(&url).spawn() {
            Ok(_) => self.status = format!("Browser geoeffnet: {query}"),
            Err(e) => self.status = format!("xdg-open fehlgeschlagen: {e}"),
        }
    }

    /// Test-Konstruktor mit injiziertem Spawner + Bin-Pfad (k8).
    #[cfg(test)]
    fn with_spawner(
        state_dir: PathBuf,
        spawn: Box<dyn Fn(&SpawnConfig) -> io::Result<Child>>,
        daemon_bin: Option<PathBuf>,
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
            daemon_bin,
            spawn,
            last_poll: Instant::now(),
            known_ts: HashSet::new(),
            filter: String::new(),
            show_log: false,
            show_knowledge: false,
            notify: false, // Tests spawnen kein notify-send
            window_size: None,
            tray_active: false,
            tray_rx: std::sync::mpsc::channel().1,
            tray_handle: None,
            hidden: false,
            quitting: false,
            tray_dirty: false,
            just_spawned: false,
        }
    }

    fn mount(&mut self) {
        // Single-Instance-UX: W4-Lock des Daemons pruefen, bevor wir einen
        // zweiten spawnen (der Daemon selbst flockt weiterhin — der Check
        // ist Meldungs-Verbesserung, nicht Schutz).
        if let Some(pid) = probe_daemon_lock(&self.dump_dir) {
            let label = pid
                .map(|p| format!(" (PID {p})"))
                .unwrap_or_default();
            self.daemon = DaemonState::Foreign { pid };
            self.status = format!("Daemon läuft extern{label} — Reports werden live angezeigt");
            self.sync_tray();
            return;
        }
        let config = match ensure_default_config(&self.state_dir) {
            Ok(c) => c,
            Err(e) => {
                self.status = format!("Config-Erzeugung fehlgeschlagen: {e}");
                return;
            }
        };
        let Some(bin) = self.daemon_bin.clone().or_else(find_daemon_bin) else {
            self.status = "crashmon Binary nicht gefunden".into();
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
                // TOCTOU-Nachlauf (s.u. logic()): nur der EIGENE Spawn
                // darf den Foreign-Wechsel ausloesen, wenn er sofort am
                // W4-Flock stirbt.
                self.just_spawned = true;
                self.status = format!("Daemon läuft (PID {})", self.daemon_pid().unwrap_or(0));
            }
            Err(e) => self.status = format!("Start fehlgeschlagen: {e}"),
        }
        // Zustandsflanke -> Tray-Label synchron (Menue-Rendering).
        self.sync_tray();
    }

    fn stop(&mut self) {
        stop_daemon(&mut self.daemon);
        self.status = "Daemon wird gestoppt (SIGTERM, Drain + Flush)...".into();
        // Zustandsflanke -> Tray-Label synchron (Menue-Rendering).
        self.sync_tray();
    }

    fn daemon_pid(&self) -> Option<u32> {
        match &self.daemon {
            DaemonState::Running { child } | DaemonState::Stopping { child, .. } => {
                Some(child.id())
            }
            DaemonState::Stopped => None,
            DaemonState::Foreign { .. } => None, // fremde PID ist nur Diagnose
        }
    }

    /// Tray-Telegramm verarbeiten. Liefert die Viewport-Commands, die der
    /// Aufrufer an egui schickt (Unit-testbar ohne echte Window-Session).
    fn handle_tray_cmd(&mut self, cmd: TrayCmd) -> Vec<egui::ViewportCommand> {
        let mut cmds = Vec::new();
        match cmd {
            TrayCmd::Show => {
                self.hidden = false;
                cmds.push(egui::ViewportCommand::Visible(true));
                // Visible allein mappt nur; Focus holt nach vorn (Wayland
                // kann verweigern — xdg-activation, dokumentiert).
                cmds.push(egui::ViewportCommand::Focus);
                cmds.push(egui::ViewportCommand::Minimized(false));
                // Alarm zuruecksetzen: der User hat jetzt hingesehen
                // (Sichtbar-Zeitpunkt-Marke der Spec). Kein D-Bus im
                // Tick, aber eine Flanke — ok. Im Test ist handle=None.
                // tray_dirty: ein im Vor-Frame gesetztes Flag (scan bei
                // hidden) wuerde das Icon im selben logic()-Tick wieder
                // auf true setzen, obwohl der User hinsieht.
                self.tray_dirty = false;
                if let Some(handle) = &self.tray_handle {
                    let _ = handle.update(|t| t.alarm = false);
                }
            }
            TrayCmd::ToggleDaemon => {
                if self.daemon.is_running() {
                    self.stop();
                } else {
                    self.mount();
                }
            }
            TrayCmd::Quit => {
                if !self.quitting {
                    self.quitting = true;
                    // Selbstheilend: verpufft Close, ist das Fenster sichtbar
                    // und quitting==true — der naechste X-Klick liefert
                    // Proceed. Absicht, nicht Zufall — nicht vereinfachen.
                    cmds.push(egui::ViewportCommand::Visible(true));
                }
                cmds.push(egui::ViewportCommand::Close);
            }
            TrayCmd::TrayLost => {
                // StatusNotifierWatcher weg: Fenster wieder zeigen — die
                // Statuszeile im versteckten Fenster sieht niemand.
                self.tray_active = false;
                self.status = "Tray verloren — Fenster wieder gezeigt, X beendet die App".into();
                // tray_dirty: hidden->sichtbar — ein im Vor-Frame gesetztes
                // Alarm-Flag (scan bei hidden) darf das Icon im selben
                // logic()-Tick nicht neu setzen.
                self.tray_dirty = false;
                if self.hidden {
                    self.hidden = false;
                    cmds.push(egui::ViewportCommand::Visible(true));
                }
            }
            TrayCmd::TrayBack => {
                self.tray_active = true;
                self.status = "Tray wieder verfügbar".into();
            }
        }
        cmds
    }

    /// DaemonState -> Tray (Menue-Label, disabled). NUR an Zustandsflanken
    /// aufrufen (handle.update = blocking D-Bus-Roundtrip).
    fn sync_tray(&mut self) {
        let Some(handle) = &self.tray_handle else { return };
        let running = self.daemon.is_running();
        let foreign = matches!(self.daemon, DaemonState::Foreign { .. });
        let _ = handle.update(|t| {
            t.daemon_running = running;
            t.daemon_foreign = foreign;
        });
    }

    /// Entprellter Poll (W1): nur alle 500 ms Verzeichnis/Log — die
    /// Daemon-Reap- und Fensterlogik laeuft pro Frame (billig).
    fn poll(&mut self, ctx: &egui::Context) {
        self.scan(ctx);
        self.log
            .refresh(&self.state_dir.join("crashmon-daemon.log"));
    }

    /// Neue Report-Dateien einsammeln (nur neue ts; Auto-Select-Regel).
    fn scan(&mut self, ctx: &egui::Context) {
        let out = scan_dir(&self.dump_dir, &self.known_ts);
        // W2-Fix: out.corrupt ist die GESAMTZAHL kaputter Dateien pro Scan —
        // saturating_add haette dieselben Dateien bei jedem Scan neu
        // gezaehlt (bei 60 fps: 3600 nach einer Minute fuer EINE Datei).
        self.corrupt = out.corrupt;
        let mut changed = false;
        for (ts, report) in out.reports {
            self.known_ts.insert(ts);
            self.reports.insert(ts, report);
            self.newest_ts = Some(ts);
            changed = true;
        }
        if changed {
            // Auto-Select nur wenn der User noch nichts gewaehlt hat — dann
            // den NEUESTEN Report (User-Auswahl wird nie ueberschrieben).
            if self.selected.is_none() {
                self.selected = self.newest_ts;
            }
            self.status = format!(
                "Report empfangen: {}",
                format_ts_local(self.newest_ts.unwrap_or(0))
            );
            if self.notify {
                self.notify_new_report();
            }
            // Alarm-Icon nur fuer Reports, die der User noch nicht gesehen
            // hat (Spec: Marke bei Hide/Show). Bei sichtbarem Fenster
            // schaut er gerade zu — kein Alarm.
            if self.hidden {
                self.tray_dirty = true; // logic() macht daraus den update()-Roundtrip
            }
            ctx.request_repaint();
        }
    }

    /// Desktop-Notification bei neuem Report (notify-send, kein Crate).
    /// Fehlschlag still: ohne notify-send (Container, Minimal-Desktop)
    /// ist die GUI selbst die Benachrichtigung.
    fn notify_new_report(&self) {
        let Some(ts) = self.newest_ts else { return };
        let Some(report) = self.reports.get(&ts) else {
            return;
        };
        let _ = std::process::Command::new("notify-send")
            .args([
                "-a",
                "crashmon",
                "-u",
                "normal",
                "crashmon",
                &summarize(report),
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }

    // --- UI-Teilmethoden (getrennt fuer Kittest) ---------------------------

    fn header_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("crashmon");
            let (label, enabled) = match self.daemon {
                DaemonState::Stopped => ("Daemon starten", true),
                DaemonState::Running { .. } => ("Daemon stoppen", true),
                DaemonState::Stopping { .. } => ("stoppt...", false),
                DaemonState::Foreign { .. } => ("läuft extern", false),
            };
            if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
                if self.daemon.is_running() {
                    self.stop();
                } else {
                    self.mount();
                }
            }
            ui.separator();
            if ui
                .selectable_label(self.show_log, "Log")
                .on_hover_text("Daemon-Log ein-/ausblenden")
                .clicked()
            {
                self.show_log = !self.show_log;
            }
            if ui
                .selectable_label(self.show_knowledge, "Wissensspeicher")
                .on_hover_text("knowledge.md in eigenem Fenster")
                .clicked()
            {
                self.show_knowledge = !self.show_knowledge;
            }
        });
    }

    /// Statuszeile in eigener Leiste unten (D5): lange Meldungen sprengen
    /// den Header nicht mehr.
    fn status_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(&self.status).weak());
            if self.corrupt > 0 {
                ui.separator();
                ui.label(
                    egui::RichText::new(format!("{} unlesbare Dateien übersprungen", self.corrupt))
                        .weak(),
                );
            }
        });
    }

    fn list_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Reports");
            ui.add(
                egui::TextEdit::singleline(&mut self.filter)
                    .hint_text("Filter...")
                    .desired_width(140.0),
            );
        });
        if !self.reports.is_empty() && self.filter.is_empty() {
            // kein separates Filter-Label — der Filter gilt nur bei Text
        }
        // auto_shrink aus: sonst ist das Content-Ui nur so breit wie der
        // laengste Titel — die Zeilenbreite (Klickflaeche + Auswahlmarke)
        // haengt dann am Inhalt statt am Panel.
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
            let now_us = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_micros() as u64)
                .unwrap_or(0);
            let filter = self.filter.trim().to_lowercase();
            // Clone statt Borrow: waehrend der Iteration wird self ggf.
            // mutiert (Loeschen per Rechtsklick).
            let reports: Vec<(u64, Report)> = self
                .reports
                .iter()
                .rev()
                .filter(|(_ts, r)| {
                    if filter.is_empty() {
                        return true;
                    }
                    summarize(r).to_lowercase().contains(&filter)
                        || format_ts_local(r.ts).to_lowercase().contains(&filter)
                })
                .map(|(ts, r)| (*ts, r.clone()))
                .collect();
            if reports.is_empty() {
                // D6: Empty-State als zentrierter Block (erster Eindruck)
                ui.add_space(24.0);
                ui.vertical_centered(|ui| {
                    if self.reports.is_empty() {
                        ui.label(egui::RichText::new("Noch keine Reports").strong());
                        ui.label(
                            egui::RichText::new(
                                "Daemon starten, dann erscheinen Crashes hier automatisch.",
                            )
                            .weak(),
                        );
                    } else {
                        ui.weak("Keine Treffer für den Filter.");
                    }
                });
            }
            for (ts, report) in reports {
                self.report_entry_ui(ui, ts, &report, now_us);
            }
        });
    }

    /// Zweizeiliger Listeneintrag (D3): Titel mit Severity-Punkt (D2),
    /// darunter relativer Zeitstempel — der volle UTC-Stempel ist die
    /// unwichtigste Info im Satz und steht in der Detailansicht.
    fn report_entry_ui(&mut self, ui: &mut egui::Ui, ts: u64, report: &Report, now_us: u64) {
        let sev = event_severity(&report.cause.kind);
        let is_new = self.newest_ts == Some(ts);
        let selected = self.selected == Some(ts);
        let color = severity_color(sev);

        let title = if is_new {
            // U+2022 statt U+25CF: der grosse Kreis fehlt in den egui-
            // Standardfonts und wird als Tofu-Kaestchen gerendert.
            format!("•  {}", summarize(report))
        } else {
            summarize(report)
        };
        let sub = format!(
            "{} · {}",
            relative_ago(report.ts, now_us),
            format_ts_local(report.ts)
        );

        // Auswahlmarke wird NACH dem Layout gezeichnet (die Zeilenhoehe steht
        // erst dann fest), aber VOR dem Text einsortiert — deshalb jetzt ein
        // Platzhalter im Painter, der unten ersetzt wird.
        let marker = ui.painter().add(egui::Shape::Noop);
        // Volle Panelbreite merken: inner.rect ist nur so breit wie der Text.
        let panel = ui.max_rect();

        let inner = ui.vertical(|ui| {
            // Titelfarbe bleibt die Severity-Farbe — auch bei Auswahl. Die
            // Severity ist die Information, die Auswahl nur ein Zustand.
            ui.label(egui::RichText::new(&title).strong().color(color));
            ui.label(egui::RichText::new(&sub).weak().small());
        });

        let row = egui::Rect::from_min_max(
            egui::pos2(panel.left(), inner.response.rect.top() - 2.0),
            egui::pos2(panel.right(), inner.response.rect.bottom() + 2.0),
        );
        let id = ui.make_persistent_id(("report-entry", ts));
        let resp = ui.interact(row, id, egui::Sense::click());

        // Auswahl = schmaler Balken links in Severity-Farbe + kaum sichtbarer
        // Hintergrund. Kein Farbblock: die Zeile soll lesbar bleiben, die
        // Marke nur sagen: diese hier.
        if selected {
            ui.painter().set(
                marker,
                egui::Shape::Vec(vec![
                    egui::Shape::rect_filled(
                        row,
                        3.0,
                        egui::Color32::from_white_alpha(SEL_BG_ALPHA),
                    ),
                    egui::Shape::rect_filled(
                        egui::Rect::from_min_max(
                            row.min,
                            egui::pos2(row.left() + SEL_BAR_WIDTH, row.bottom()),
                        ),
                        1.0,
                        color,
                    ),
                ]),
            );
        } else if resp.hovered() {
            // Nur Hover-Andeutung (die ganze Zeile ist Klickflaeche).
            ui.painter().set(
                marker,
                egui::Shape::rect_filled(row, 3.0, egui::Color32::from_white_alpha(6)),
            );
        }
        if resp.clicked() {
            self.selected = Some(ts);
        }
        if resp.secondary_clicked() {
            // D6: Reports loeschen (Datei + Liste) — einziger Weg bisher
            // war der Dateimanager.
            let path = self.dump_dir.join(format!("crash-{ts}.json"));
            let _ = std::fs::remove_file(&path);
            self.reports.remove(&ts);
            self.known_ts.remove(&ts);
            if self.selected == Some(ts) {
                self.selected = None;
            }
            self.status = format!("Report {ts} geloescht");
        }
        ui.add_space(2.0);
    }

    fn detail_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Detail");
            if let Some(ts) = self.selected {
                ui.weak(format_ts_local(ts));
            }
            if ui.button("JSON kopieren").clicked() {
                if let Some(report) = self.selected.and_then(|ts| self.reports.get(&ts)) {
                    let json = serde_json::to_string_pretty(report).unwrap_or_default();
                    ui.output_mut(|o| {
                        o.commands.push(egui::output::OutputCommand::CopyText(json));
                    });
                    self.status = "Report-JSON kopiert".into();
                }
            }
        });
        let Some(report) = self.selected.and_then(|ts| self.reports.get(&ts).cloned()) else {
            ui.add_space(24.0);
            ui.vertical_centered(|ui| {
                ui.weak("Report in der Liste auswählen.");
            });
            return;
        };
        // D2: Severity-Farbkopf — die Gewichtung, ohne dass man lesen muss.
        let sev = event_severity(&report.cause.kind);
        ui.horizontal(|ui| {
            ui.colored_label(severity_color(sev), format!("Severity: {sev}"));
            if report.lost_events > 0 {
                ui.weak(format!("{} Events verloren", report.lost_events));
            }
        });
        ui.separator();
        ui.strong("Ursache");
        render_event(ui, &report.cause);
        if !report.related.is_empty() {
            ui.separator();
            ui.strong(format!("Beigeordnet ({})", report.related.len()));
            for ev in &report.related {
                render_event(ui, ev);
            }
        }

        // Eingebaute Wissensbasis (Xid-Severity, Event-Erklaerungen)
        ui.separator();
        ui.strong("Referenz");
        for (title, text) in reference_lines(&report.cause.kind) {
            reference_paragraph(ui, &title, &text);
        }
        // k1/Umbau: manuelles Nachschlagen im Browser (statt DDG-API) —
        // der User entscheidet, was er in die Wissensdatei uebernimmt.
        if let EventKind::GpuXid { code, .. } = &report.cause.kind
            && ui.button(format!("Xid {code} im Browser suchen")).clicked()
        {
            self.open_browser_search(&format!("NVRM Xid {code}"));
        }
    }

    fn log_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.strong("Daemon-Log");
            ui.weak(format!("({} Zeilen Tail)", self.log.lines().count()));
            if ui.button("Schließen").clicked() {
                self.show_log = false;
            }
        });
        egui::ScrollArea::vertical()
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for line in self.log.lines() {
                    ui.monospace(line);
                }
            });
    }

    /// D5: Wissensspeicher in eigenem Fenster — schiebt den Report nicht
    /// mehr aus der Detailspalte und wird als Markdown (proportional,
    /// umbrechend) statt Monospace-Zeilen gerendert.
    fn knowledge_window(&mut self, ctx: &egui::Context) {
        if !self.show_knowledge {
            return;
        }
        let mut open = self.show_knowledge;
        egui::Window::new("Wissensspeicher (knowledge.md)")
            .open(&mut open)
            .default_size([560.0, 420.0])
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.add(egui::Label::new(&self.knowledge).wrap_mode(egui::TextWrapMode::Wrap));
                });
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("Neu laden").clicked() {
                        self.refresh_knowledge();
                    }
                    ui.weak(format!(
                        "Editierbar: {}",
                        self.state_dir.join("knowledge.md").display()
                    ));
                });
            });
        self.show_knowledge = open;
    }
}

/// Referenz-Absatz: Titel (schwach) + Fliesstext als EIN Label. Vorher
/// standen beide als getrennte Widgets in `horizontal_wrapped` — dort erbt
/// ein Label `TextWrapMode::Extend` und laeuft rechts aus dem Panel heraus,
/// statt an der Kante umzubrechen.
fn reference_paragraph(ui: &mut egui::Ui, title: &str, text: &str) {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = ui.available_width();
    job.append(
        &format!("{title}: "),
        0.0,
        egui::TextFormat {
            font_id: font.clone(),
            color: ui.visuals().weak_text_color(),
            ..Default::default()
        },
    );
    job.append(
        text,
        0.0,
        egui::TextFormat {
            font_id: font,
            color: ui.visuals().text_color(),
            ..Default::default()
        },
    );
    ui.add(egui::Label::new(job));
    ui.add_space(2.0);
}

/// Technischer Wert (Pfad, Kernel-Meldung): an der Spaltenkante mit Ellipse
/// gekuerzt statt aus dem Fenster zu laufen. Volltext im Tooltip — Umbruch
/// waere hier schlechter, ein dreizeiliger Core-Pfad sprengt das Grid.
fn mono_clip(ui: &mut egui::Ui, value: &str) {
    ui.add(
        egui::Label::new(egui::RichText::new(value).monospace())
            .wrap_mode(egui::TextWrapMode::Truncate),
    )
    .on_hover_text(value);
}

/// Formatiertes Event-Detail (D1): Grid mit schwachen Labels + betonten
/// Werten — Monospace nur noch fuer technische Werte (Pfade, Meldungen),
/// keine handgezaehlten Leerzeichen mehr.
fn render_event(ui: &mut egui::Ui, ev: &CrashEvent) {
    // Deckel fuer die Wertespalte: ohne ihn richtet sich die Grid-Breite
    // nach dem laengsten Wert (Core-Pfad) und schiebt ihn aus dem Panel.
    let value_max = (ui.available_width() - 120.0).max(160.0);
    egui::Grid::new(format!("event-{}", ev.ts))
        .num_columns(2)
        .spacing([16.0, 4.0])
        .max_col_width(value_max)
        .show(ui, |ui| {
            ui.weak("Zeit");
            ui.label(format_ts_local(ev.ts));
            ui.end_row();
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
                    ui.weak("Art");
                    ui.label("Coredump");
                    ui.end_row();
                    ui.weak("PID");
                    ui.monospace(pid.to_string());
                    ui.end_row();
                    if let Some(exe) = exe {
                        ui.weak("Programm");
                        mono_clip(ui, exe);
                        ui.end_row();
                    }
                    ui.weak("Comm");
                    ui.label(comm);
                    ui.end_row();
                    if let Some(s) = signal {
                        ui.weak("Signal");
                        ui.strong(s);
                        ui.end_row();
                    }
                    if let Some(u) = uid {
                        ui.weak("UID");
                        ui.monospace(u.to_string());
                        ui.end_row();
                    }
                    if let Some(u) = unit {
                        ui.weak("Unit");
                        ui.label(u);
                        ui.end_row();
                    }
                    if let Some(f) = coredump_file {
                        ui.weak("Core-Datei");
                        mono_clip(ui, f);
                        ui.end_row();
                    }
                }
                EventKind::OomKill { pid, comm } => {
                    ui.weak("Art");
                    ui.label("OOM-Kill");
                    ui.end_row();
                    ui.weak("PID");
                    ui.monospace(pid.to_string());
                    ui.end_row();
                    ui.weak("Comm");
                    ui.label(comm);
                    ui.end_row();
                }
                EventKind::GpuXid {
                    code,
                    pci,
                    pid,
                    message,
                } => {
                    ui.weak("Art");
                    ui.label(format!("NVIDIA Xid {code}"));
                    ui.end_row();
                    if let Some(p) = pci {
                        ui.weak("PCI");
                        ui.monospace(p);
                        ui.end_row();
                    }
                    if let Some(p) = pid {
                        ui.weak("PID");
                        ui.monospace(p.to_string());
                        ui.end_row();
                    }
                    ui.weak("Meldung");
                    mono_clip(ui, message);
                    ui.end_row();
                }
                EventKind::GpuReset { vendor, detail } => {
                    ui.weak("Art");
                    ui.label(format!("GPU Reset ({vendor})"));
                    ui.end_row();
                    ui.weak("Meldung");
                    mono_clip(ui, detail);
                    ui.end_row();
                }
                EventKind::GpuWedged { method, device } => {
                    ui.weak("Art");
                    ui.label("GPU Wedged");
                    ui.end_row();
                    if let Some(m) = method {
                        ui.weak("Methode");
                        ui.monospace(m);
                        ui.end_row();
                    }
                    if let Some(d) = device {
                        ui.weak("Device");
                        mono_clip(ui, d);
                        ui.end_row();
                    }
                }
            }
        });
}

/// D2: Severity-Farben (dunkles Theme): fatal rot, kritisch orange,
/// hoch gelb, mittel blaugrau, unbekannt grau.
fn severity_color(sev: Severity) -> egui::Color32 {
    match sev {
        Severity::Fatal => egui::Color32::from_rgb(0xE5, 0x48, 0x4F),
        Severity::Kritisch => egui::Color32::from_rgb(0xE8, 0x8C, 0x30),
        Severity::Hoch => egui::Color32::from_rgb(0xD8, 0xB4, 0x38),
        Severity::Mittel => egui::Color32::from_rgb(0x8A, 0xB4, 0xC8),
        Severity::Unbekannt => egui::Color32::from_gray(0x80),
    }
}

/// Minimaler URL-Encoder fuer Suchanfragen (Leerzeichen -> %20).
fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b' ' => out.push_str("%20"),
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crash_daemon::event::EventKind;
    use std::fs;

    #[test]
    fn close_action_tabelle() {
        use CloseAction::*;
        assert_eq!(close_action(false, false), Proceed, "kein Tray -> X beendet");
        assert_eq!(close_action(false, true), Proceed);
        assert_eq!(close_action(true, false), Hide, "Tray -> X versteckt");
        assert_eq!(close_action(true, true), Proceed, "Tray-Quit -> raus");
    }

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
                    device: None,
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
        let mut app = CrashmonGui::with_spawner(dir.clone(), Box::new(fake_spawner), None);
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
    fn scan_counts_corrupt_files_exactly_once() {
        // W2-Regression: out.corrupt ist die Gesamtzahl pro Scan — der
        // Zaehler darf beim zweiten Scan nicht nochmal hochgehen.
        let dir = temp_state("corrupt");
        write_report(&dir, 1_000_000);
        fs::write(dir.join("crash-9999999999999.json"), "garbage").unwrap();
        let mut app = CrashmonGui::with_spawner(dir.clone(), Box::new(fake_spawner), None);
        app.dump_dir = dir.clone();

        let ctx = egui::Context::default();
        app.scan(&ctx);
        assert_eq!(app.reports.len(), 1, "gueltiger bleibt");
        assert_eq!(app.corrupt, 1, "eine kaputte Datei");

        // Zweiter Scan: immer noch 1 (kein saturating_add der Summe)
        app.scan(&ctx);
        assert_eq!(app.corrupt, 1, "kein Doppelzaehlen");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn kittest_click_mount_button() {
        // Headless-UI-Smoke (egui_kittest, kein Display noetig):
        // Klick auf "Daemon starten" -> Zustand Running + Label wechselt.
        let dir = temp_state("kittest");
        let app = CrashmonGui::with_spawner(
            dir.clone(),
            Box::new(fake_spawner),
            Some(dir.join("crashmon")), // k8: kein PATH-set_var mehr
        );
        let mut harness =
            egui_kittest::Harness::new_ui_state(|ui, app: &mut CrashmonGui| app.header_ui(ui), app);

        // Klick auf den Start-Button (AccessKit-Label; Queryable-Trait in Scope)
        use egui_kittest::kittest::Queryable;
        harness.get_by_label("Daemon starten").click();
        harness.run();
        assert!(
            harness.state().daemon.is_running(),
            "Klick muss Daemon starten (status: {})",
            harness.state().status
        );

        // Aufraeumen
        let mut app = harness.into_state();
        shutdown_daemon(&mut app.daemon, Duration::from_secs(3));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn mount_externen_daemon_erkennt_als_foreign() {
        use std::os::unix::io::AsRawFd; // libc::flock braucht den FD
        let dir = temp_state("foreign");
        // Lock von aussen belegen (simuliert fremden Daemon)
        let file = fs::File::create(dir.join(".lock")).unwrap();
        unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        let mut app = CrashmonGui::with_spawner(dir.clone(), Box::new(fake_spawner), None);
        app.mount();
        assert!(
            matches!(app.daemon, DaemonState::Foreign { .. }),
            "mount darf keinen zweiten Daemon spawnen (status: {})",
            app.status
        );
        assert!(
            app.status.contains("läuft extern"),
            "Status zeigt externen Daemon: {}",
            app.status
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn mount_sets_running_with_fake_spawner() {
        let dir = temp_state("mount");
        let mut app = CrashmonGui::with_spawner(
            dir.clone(),
            Box::new(fake_spawner),
            Some(dir.join("crashmon")), // k8: Bin-Pfad injiziert, kein PATH
        );
        app.mount();
        assert!(app.daemon.is_running(), "status: {}", app.status);
        assert!(app.status.contains("PID"), "status: {}", app.status);

        shutdown_daemon(&mut app.daemon, Duration::from_secs(3));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn tray_show_und_quit_steuern_fenster() {
        let dir = temp_state("traycmd");
        let mut app = CrashmonGui::with_spawner(dir.clone(), Box::new(fake_spawner), None);
        app.hidden = true;
        app.tray_dirty = true; // F2: Vor-Frame-Flag (scan bei hidden)
        let cmds = app.handle_tray_cmd(TrayCmd::Show);
        assert!(!app.hidden, "Show macht sichtbar");
        assert!(!app.tray_dirty, "Show konsumiert das Alarm-Flag");
        assert!(cmds.iter().any(|c| matches!(c, egui::ViewportCommand::Visible(true))));
        assert!(cmds.iter().any(|c| matches!(c, egui::ViewportCommand::Focus)));

        app.handle_tray_cmd(TrayCmd::Quit);
        assert!(app.quitting, "Quit setzt quitting");
        let cmds = app.handle_tray_cmd(TrayCmd::Quit);
        assert!(cmds.iter().any(|c| matches!(c, egui::ViewportCommand::Close)));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn tray_lost_zeigt_fenster_wieder() {
        let dir = temp_state("traystraylost");
        let mut app = CrashmonGui::with_spawner(dir.clone(), Box::new(fake_spawner), None);
        app.tray_active = true;
        app.hidden = true;
        app.tray_dirty = true; // F2: Vor-Frame-Flag (scan bei hidden)
        let cmds = app.handle_tray_cmd(TrayCmd::TrayLost);
        assert!(!app.tray_active, "TrayLost deaktiviert Tray-Modus");
        assert!(!app.tray_dirty, "TrayLost konsumiert das Alarm-Flag");
        assert!(!app.hidden, "TrayLost macht das Fenster wieder sichtbar");
        assert!(
            cmds.iter().any(|c| matches!(c, egui::ViewportCommand::Visible(true))),
            "verlorenes Tray -> Fenster zeigen"
        );
        assert!(app.status.contains("Tray verloren"), "{}", app.status);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn tray_disconnected_zeigt_fenster_wieder() {
        // F5-Regression: stirbt der ksni-Thread (Service-Exit/Panic/Bus-
        // Ausfall ohne watcher_offline), faellt try_recv auf Disconnected —
        // logic() muss dann TrayLost-Logik fahren, sonst bleibt die GUI
        // versteckt und unerreichbar.
        use eframe::App; // logic() ist eframe::App-Methode
        let dir = temp_state("traydisc");
        let (tx, rx) = std::sync::mpsc::channel::<TrayCmd>();
        drop(tx); // Sender (ksni-Thread) tot
        let mut app = CrashmonGui::with_spawner(dir.clone(), Box::new(fake_spawner), None);
        app.tray_rx = rx;
        app.tray_active = true;
        app.hidden = true;
        let ctx = egui::Context::default();
        // _new_kittest: einziger public Frame-Konstruktor (Test-Harness).
        app.logic(&ctx, &mut eframe::Frame::_new_kittest());
        assert!(!app.tray_active, "Disconnected deaktiviert Tray-Modus");
        assert!(!app.hidden, "Disconnected zeigt das Fenster wieder");
        assert!(app.status.contains("Tray verloren"), "{}", app.status);
        fs::remove_dir_all(&dir).ok();
    }
}

impl eframe::App for CrashmonGui {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(Duration::from_secs(1));
        // Tray-Telegramme (Show/Quit/Toggle/TrayLost/TrayBack). Der
        // Watcher-Verlust kommt als NATIVES ksni-Event (watcher_offline
        // im D-Bus-Thread -> Sender) — kein is_closed-Poll noetig
        // (Review: Handle-Closed ist nicht dasselbe wie Host-Wegfall;
        // watcher_offline feuert bei StatusNotifierWatcher-Exit).
        loop {
            match self.tray_rx.try_recv() {
                Ok(cmd) => {
                    for c in self.handle_tray_cmd(cmd) {
                        ctx.send_viewport_cmd(c);
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    // ksni-Thread tot (Service-Exit/Panic/Bus-Ausfall ohne
                    // watcher_offline): sonst bleibt die GUI versteckt und
                    // unerreichbar — TrayLost-Logik, einmal.
                    if self.tray_active {
                        self.tray_active = false;
                        // Split-Form (fmt: Zeile >100, identisch zu TrayLost).
                        self.status =
                            "Tray verloren — Fenster wieder gezeigt, X beendet die App".into();
                        if self.hidden {
                            self.hidden = false;
                            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                        }
                    }
                    break;
                }
            }
        }
        // Alarm-Icon-Flanke: EIN Roundtrip, nur wenn scan() Neues fand
        // (dirty). Bewusst ausserhalb des Debounce-Zweigs.
        if self.tray_dirty {
            self.tray_dirty = false;
            if let Some(handle) = &self.tray_handle {
                let _ = handle.update(|t| t.alarm = true);
            }
        }
        // X-Klick: Hide (Tray) oder Proceed (App endet -> on_exit)
        if ctx.input(|i| i.viewport().close_requested()) {
            match close_action(self.tray_active, self.quitting) {
                CloseAction::Hide => {
                    self.hidden = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                }
                CloseAction::Proceed => { /* App beendet nach diesem Frame */ }
            }
        }
        // D6: nur bei sichtbarem Fenster — ein verstecktes Viewport meldet
        // keine garantierte letzte sichtbare Groesse ("Briefmarken"-Bug).
        if !self.hidden {
            self.window_size = ctx.input(|i| i.viewport().inner_rect.map(|r| r.size()));
            if let Some(size) = self.window_size {
                ctx.data_mut(|d| {
                    d.insert_persisted(egui::Id::new(WINDOW_SIZE_KEY), size);
                });
            }
        }
        // Daemon-Reap: billig (try_wait), darf pro Frame laufen — kein
        // Zombie, auch bei verstecktem Log/Fenster.
        if let Some(msg) = poll_daemon(&mut self.daemon) {
            // TOCTOU: zwei GUI-Starts — der Verlierer-Daemon stirbt sofort
            // am W4-Flock. Dann ist der Sieger im Lock: Foreign statt
            // generischer "exit status"-Meldung.
            if self.just_spawned {
                self.just_spawned = false;
                if let Some(pid) = probe_daemon_lock(&self.dump_dir).flatten()
                    && matches!(self.daemon, DaemonState::Stopped)
                {
                    self.daemon = DaemonState::Foreign { pid: Some(pid) };
                    self.status =
                        "Daemon läuft extern — Reports werden live angezeigt".into();
                } else {
                    self.status = msg;
                }
            } else {
                self.status = msg;
            }
            // Zustandsflanke -> Tray-Label synchron (Menue-Rendering).
            self.sync_tray();
            ctx.request_repaint();
        }
        // W1: Scan/Log entprellt — nicht an der Framerate.
        if self.last_poll.elapsed() >= POLL_INTERVAL {
            self.last_poll = Instant::now();
            // Foreign-Auto-Release NUR im Foreign-Zweig: Lock erneut
            // proben; frei -> Stopped + "Bereit"-Meldung. poll_foreign
            // reapt kein Kind (gibt keins), also kein Running/Stopping-
            // Eingriff moeglich.
            if matches!(self.daemon, DaemonState::Foreign { .. })
                && let Some(msg) = poll_foreign(&mut self.daemon, &self.dump_dir)
            {
                self.status = msg;
                // Zustandsflanke -> Tray-Label synchron (Menue-Rendering).
                self.sync_tray();
            }
            self.poll(ctx);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::top("header").show(ui, |ui| self.header_ui(ui));
        egui::Panel::bottom("status")
            .default_size(24.0)
            .show(ui, |ui| self.status_ui(ui));
        if self.show_log {
            egui::Panel::bottom("log")
                .resizable(true)
                .default_size(160.0)
                .show(ui, |ui| self.log_ui(ui));
        }
        egui::Panel::left("reports")
            .default_size(340.0)
            .show(ui, |ui| self.list_ui(ui));
        egui::CentralPanel::default().show(ui, |ui| self.detail_ui(ui));
        self.knowledge_window(ui.ctx());
    }

    fn on_exit(&mut self) {
        // Fenster zu -> Daemon immer mitbeenden (kein Waisen-Prozess).
        // Blocking ist hier ok (Fenster bereits geschlossen). Die
        // Fenstergroesse persistiert egui selbst (insert_persisted in logic).
        shutdown_daemon(&mut self.daemon, Duration::from_secs(3));
    }
}
