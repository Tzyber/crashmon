//! Systemtray (StatusNotifierItem via ksni, blocking).
//!
//! Menue-Callbacks laufen im ksni-D-Bus-Thread und machen NUR
//! Sender-Push — die eigentliche Arbeit (Fenster zeigen, Daemon
//! starten/stoppen, Beenden) macht der GUI-Thread in logic().
//! Der Rueckweg GUI->Tray laeuft ueber `Handle::update` — NUR an
//! Zustandsflanken (D-Bus-Roundtrip, blocking).

use ksni::menu::{MenuItem, StandardItem};
use ksni::{Icon, Tray};

/// GUI-Thread-Anweisungen aus dem Tray-Menue.
#[derive(Debug, PartialEq)]
pub enum TrayCmd {
    Show,
    ToggleDaemon,
    Quit,
    /// Watcher offline (z. B. Plasma-Neustart) — GUI entscheidet:
    /// Fenster zeigen, tray_active=false.
    TrayLost,
    /// Watcher wieder da (nur nach TrayLost, laut ksni-Doku).
    TrayBack,
}

pub struct CrashmonTray {
    sender: std::sync::mpsc::Sender<TrayCmd>,
    pub alarm: bool,
    pub daemon_running: bool,
    pub daemon_foreign: bool,
}

impl CrashmonTray {
    pub fn new(sender: std::sync::mpsc::Sender<TrayCmd>) -> Self {
        Self {
            sender,
            alarm: false,
            daemon_running: false,
            daemon_foreign: false,
        }
    }
}

impl Tray for CrashmonTray {
    fn id(&self) -> String {
        "crashmon-gui".into()
    }
    fn title(&self) -> String {
        "crashmon — Crash-Daemon".into()
    }
    /// Absichtlich leer (SNI: Name hat Vorrang vor Pixmap; unbekannter
    /// Name -> "gar kein Icon" auf manchen Hosts). Das eingebettete
    /// Pixmap ist die einzige Icon-Quelle.
    fn icon_name(&self) -> String {
        String::new()
    }
    fn icon_pixmap(&self) -> Vec<Icon> {
        crash_icon(self.alarm)
    }
    /// Icon-Klick zeigt das Fenster (Standard-Verhalten).
    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self.sender.send(TrayCmd::Show);
    }
    /// Watcher (StatusNotifierHost-Aggregat) ist offline — z. B. Plasma-
    /// Neustart, GNOME-Extension weg. Laeuft im ksni-Thread: NUR Sender-
    /// Push, die GUI entscheidet (Fenster zeigen, tray_active=false).
    /// Rueckgabe true: Service weiterlaufen lassen, damit `watcher_online`
    /// nach dem Neustart wieder greifen kann.
    fn watcher_offline(&self, _reason: ksni::OfflineReason) -> bool {
        let _ = self.sender.send(TrayCmd::TrayLost);
        true
    }
    /// Watcher wieder da (nur nach watcher_offline, laut ksni-Doku).
    fn watcher_online(&self) {
        let _ = self.sender.send(TrayCmd::TrayBack);
    }
    fn menu(&self) -> Vec<MenuItem<Self>> {
        let sender = self.sender.clone();
        let toggle_sender = self.sender.clone();
        let quit_sender = self.sender.clone();
        let (toggle_label, toggle_enabled) = if self.daemon_foreign {
            ("Daemon starten".to_string(), false)
        } else if self.daemon_running {
            ("Daemon stoppen".to_string(), true)
        } else {
            ("Daemon starten".to_string(), true)
        };
        vec![
            StandardItem {
                label: "Fenster anzeigen".into(),
                activate: Box::new(move |_this: &mut Self| {
                    let _ = sender.send(TrayCmd::Show);
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: toggle_label,
                enabled: toggle_enabled,
                activate: Box::new(move |_this: &mut Self| {
                    let _ = toggle_sender.send(TrayCmd::ToggleDaemon);
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Beenden".into(),
                icon_name: "application-exit".into(),
                // move: Box<dyn Fn> ist 'static — ohne move wuerde der
                // Borrow von quit_sender nur bis zum Ende von menu()
                // leben (Compiler: "dropped here while still borrowed").
                activate: Box::new(move |_this: &mut Self| {
                    let _ = quit_sender.send(TrayCmd::Quit);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

// -- Icon-Generator ------------------------------------------------------

/// Eingebettetes 16+22-px-Icon (ARGB32, big-endian laut SNI-Spec — falls
/// Farben vertauscht ankommen, ist das die Ursache, nicht der Generator).
/// Neutral: grauer Kreis; Alarm: roter Kreis — beide mit weissem
/// Ausrufezeichen (Balken + Punkt, Spezifikation).
pub fn crash_icon(alarm: bool) -> Vec<Icon> {
    [16u32, 22u32]
        .iter()
        .map(|&size| {
            let mut data = vec![0u8; (size * size * 4) as usize];
            // Normalisierte Pixel-Koordinaten in [-0.5, 0.5] (y nach unten).
            let (cr, cg, cb) = if alarm {
                (0xE5u8, 0x48u8, 0x4Fu8) // Alarm: rot
            } else {
                (0x8Au8, 0x8Au8, 0x8Au8) // neutral: grau
            };
            for y in 0..size {
                for x in 0..size {
                    let px = (x as f32 + 0.5) / size as f32 - 0.5;
                    let py = (y as f32 + 0.5) / size as f32 - 0.5;
                    let in_circle = px * px + py * py <= 0.42 * 0.42;
                    let in_bar = px.abs() <= 0.07 && (-0.22..=0.10).contains(&py);
                    let in_dot = px * px + (py - 0.25) * (py - 0.25) <= 0.09 * 0.09;
                    if in_circle {
                        let idx = ((y * size + x) * 4) as usize;
                        data[idx] = 0xFF; // A
                        if in_bar || in_dot {
                            data[idx + 1] = 0xFF; // weisses Ausrufezeichen
                            data[idx + 2] = 0xFF;
                            data[idx + 3] = 0xFF;
                        } else {
                            data[idx + 1] = cr;
                            data[idx + 2] = cg;
                            data[idx + 3] = cb;
                        }
                    }
                }
            }
            Icon {
                // ksni 0.3.6: width/height sind i32 (nicht u32).
                width: size as i32,
                height: size as i32,
                data,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ksni::menu::MenuItem;
    // Tray-Trait in Scope: menu()/activate()/watcher_* sind Trait-Methoden.
    use ksni::Tray;

    #[test]
    fn menu_sendet_commands() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut tray = CrashmonTray::new(tx);
        let mut menu = tray.menu();
        // Reihenfolge: Fenster anzeigen, Daemon-Toggle, Beenden
        assert_eq!(menu.len(), 3);
        // activate ist Box<dyn Fn> — der Aufruf braucht &mut (nicht &).
        let MenuItem::Standard(fenster) = &mut menu[0] else {
            panic!("erstes Item")
        };
        assert_eq!(fenster.label, "Fenster anzeigen");
        (fenster.activate)(&mut tray);
        assert_eq!(rx.try_recv().unwrap(), TrayCmd::Show);

        let MenuItem::Standard(toggle) = &mut menu[1] else {
            panic!("zweites Item")
        };
        assert!(toggle.enabled, "Toggle enabled bei laufendem Daemon");
        (toggle.activate)(&mut tray);
        assert_eq!(rx.try_recv().unwrap(), TrayCmd::ToggleDaemon);

        let MenuItem::Standard(quit) = &mut menu[2] else {
            panic!("drittes Item")
        };
        (quit.activate)(&mut tray);
        assert_eq!(rx.try_recv().unwrap(), TrayCmd::Quit);
    }

    #[test]
    fn toggle_label_und_foreign() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut tray = CrashmonTray::new(tx);
        tray.daemon_running = true;
        let mut menu = tray.menu();
        let MenuItem::Standard(toggle) = &mut menu[1] else {
            panic!()
        };
        assert_eq!(toggle.label, "Daemon stoppen");
        tray.daemon_running = false;
        tray.daemon_foreign = true;
        let mut menu = tray.menu();
        let MenuItem::Standard(toggle) = &mut menu[1] else {
            panic!()
        };
        assert_eq!(toggle.label, "Daemon starten");
        assert!(!toggle.enabled, "Foreign -> disabled");
    }

    #[test]
    fn activate_sendet_show() {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut tray = CrashmonTray::new(tx);
        tray.activate(0, 0);
        assert_eq!(
            rx.try_recv().unwrap(),
            TrayCmd::Show,
            "Icon-Klick zeigt Fenster"
        );
    }

    #[test]
    fn watcher_offline_sendet_traylost_und_bleibt_am_leben() {
        let (tx, rx) = std::sync::mpsc::channel();
        let tray = CrashmonTray::new(tx);
        // watcher_offline laeuft im ksni-Thread; hier testen wir den Sender-Push.
        let keep = tray.watcher_offline(ksni::OfflineReason::No);
        assert!(keep, "true = Service weiterlaufen lassen (GUI entscheidet)");
        assert_eq!(rx.try_recv().unwrap(), TrayCmd::TrayLost);
    }

    #[test]
    fn watcher_online_sendet_trayback() {
        let (tx, rx) = std::sync::mpsc::channel();
        let tray = CrashmonTray::new(tx);
        tray.watcher_online();
        assert_eq!(rx.try_recv().unwrap(), TrayCmd::TrayBack);
    }

    #[test]
    fn icon_pixmap_alarm_unterscheidet_sich() {
        let neutral = crash_icon(false);
        let alarm = crash_icon(true);
        assert_eq!(neutral.len(), 2, "16px + 22px");
        assert_eq!(alarm.len(), 2);
        assert_ne!(neutral[0].data, alarm[0].data, "Alarm != neutral");
        assert!(neutral[0].data.iter().any(|b| *b != 0), "nicht leer");
        assert_eq!(neutral[0].width, 16);
        assert_eq!(neutral[0].height, 16);
    }
}
