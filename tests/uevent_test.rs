//! Uevent-Parser-Fixtures (Phase 2.3, TDD).
//!
//! Format: `KEY=VALUE`-Zeilen, NUL-getrennt. Filter: SUBSYSTEM=drm UND
//! WEDGED= gesetzt (Linux ~6.14/6.15, recherche-phase1.md 2.4).
//! Deterministische Parser-Tests; der Netlink-Listener selbst wird im
//! ignored Smoke-Test geprueft (Setup + Drain; echte Uevent-Injektion ist
//! root-only — Senden an KOBJECT_UEVENT braucht CAP_SYS_ADMIN).

use crash_daemon::event::EventKind;
use crash_daemon::gpu::uevent::parse_uevent;

/// Baut einen Uevent-Buffer aus KEY=VALUE-Zeilen (NUL-getrennt).
fn uevent(entries: &[&str]) -> Vec<u8> {
    let mut buf = Vec::new();
    for e in entries {
        buf.extend_from_slice(e.as_bytes());
        buf.push(0);
    }
    buf
}

const WEDGED_BUS_RESET: &[&str] = &[
    "ACTION=change",
    "DEVPATH=/devices/pci0000:00/0000:00:02.0/drm/card0",
    "SUBSYSTEM=drm",
    "WEDGED=bus-reset",
    "DEVTYPE=drm_minor",
    "SEQNUM=12345",
];

#[test]
fn wedged_bus_reset() {
    let kind = parse_uevent(&uevent(WEDGED_BUS_RESET)).expect("wedged event");
    assert_eq!(
        kind,
        EventKind::GpuWedged {
            method: Some("bus-reset".into())
        }
    );
}

#[test]
fn wedged_rebind() {
    let msg = ["ACTION=change", "SUBSYSTEM=drm", "WEDGED=rebind"];
    assert_eq!(
        parse_uevent(&uevent(&msg)),
        Some(EventKind::GpuWedged {
            method: Some("rebind".into())
        })
    );
}

#[test]
fn wedged_plain() {
    // WEDGED ohne Wert (leer) — trotzdem ein Wedge-Signal, Methode unbekannt
    let msg = ["ACTION=change", "SUBSYSTEM=drm", "WEDGED="];
    assert_eq!(
        parse_uevent(&uevent(&msg)),
        Some(EventKind::GpuWedged { method: None })
    );
}

#[test]
fn drm_without_wedged_is_none() {
    // Normale DRM-Hotplug-Events (Monitor an/ab) sind KEINE Wedges
    let msg = [
        "ACTION=change",
        "SUBSYSTEM=drm",
        "DEVTYPE=drm_minor",
        "HOTPLUG=1",
    ];
    assert_eq!(parse_uevent(&uevent(&msg)), None);
}

#[test]
fn non_drm_subsystem_is_none() {
    // Netzwerk-/USB-Uevents interessieren nicht
    let msg = ["ACTION=add", "SUBSYSTEM=net", "INTERFACE=eth0"];
    assert_eq!(parse_uevent(&uevent(&msg)), None);
}

#[test]
fn wedged_non_drm_is_none() {
    // WEDGED nur bei SUBSYSTEM=drm relevant
    let msg = ["ACTION=change", "SUBSYSTEM=block", "WEDGED=reboot"];
    assert_eq!(parse_uevent(&uevent(&msg)), None);
}

#[test]
fn missing_action_is_none() {
    // Ohne ACTION kein gueltiges Uevent
    let msg = ["SUBSYSTEM=drm", "WEDGED=rebind"];
    assert_eq!(parse_uevent(&uevent(&msg)), None);
}

#[test]
fn garbage_buffer_is_none() {
    assert_eq!(parse_uevent(b"kein key value format ohne nul\x00"), None);
}

#[test]
fn empty_buffer_is_none() {
    assert_eq!(parse_uevent(b""), None);
}

// --- Edge-Cases (Review 2.3) ---------------------------------------------

#[test]
fn duplicate_key_last_wins() {
    // Letztes Vorkommen gewinnt (wie bei libudev/Uevent-Semantik)
    let msg = [
        "ACTION=change",
        "SUBSYSTEM=drm",
        "SUBSYSTEM=net",
        "WEDGED=rebind",
    ];
    assert_eq!(parse_uevent(&uevent(&msg)), None, "SUBSYSTEM=net gewinnt");
}

#[test]
fn equals_sign_in_value() {
    let msg = ["ACTION=change", "SUBSYSTEM=drm", "WEDGED=a=b"];
    assert_eq!(
        parse_uevent(&uevent(&msg)),
        Some(EventKind::GpuWedged {
            method: Some("a=b".into())
        })
    );
}

#[test]
fn no_trailing_nul() {
    // Abgeschnittene Nachricht: letztes Feld parst trotzdem
    let msg = ["ACTION=change", "SUBSYSTEM=drm", "WEDGED=reboot"];
    let mut buf = uevent(&msg);
    buf.pop(); // schliessendes NUL entfernen
    assert_eq!(
        parse_uevent(&buf),
        Some(EventKind::GpuWedged {
            method: Some("reboot".into())
        })
    );
}

#[test]
fn nlmsg_noop_prefix_is_skipped() {
    // Root-injizierte udev-Nachrichten tragen einen nlmsghdr-Praeﬁx
    // (NLMSG_NOOP); der Parser ueberspringt die Bytes als "Zeile ohne =".
    let mut buf = vec![0u8; 16]; // fiktiver nlmsghdr (POD-Zero)
    buf.extend_from_slice(&uevent(WEDGED_BUS_RESET));
    assert_eq!(
        parse_uevent(&buf),
        Some(EventKind::GpuWedged {
            method: Some("bus-reset".into())
        })
    );
}
