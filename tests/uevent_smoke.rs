//! Netlink-Uevent-Smoke (Phase 2.3).
//!
//! Verifiziert Setup (Socket, Bind an Gruppe 1, RCVBUF) und
//! Drain-Logik (`read_events` bis EAGAIN). Echte Uevent-Injektion ist
//! root-only (Kernel: Senden an KOBJECT_UEVENT braucht CAP_SYS_ADMIN,
//! `NL_CFG_F_NONROOT_RECV` ohne `NONROOT_SEND` seit ~4.10) — den realen
//! WEDGED-Empfang deckt das udevadm-Rezept in Phase 2.5 ab.
//!
//! Ignored: netzwerkartig/Systemzustand-abhaengig. Lokal:
//! `cargo test --test uevent_smoke -- --ignored --nocapture`.

use crash_daemon::gpu::uevent::GpuUeventListener;

#[test]
#[ignore]
fn setup_and_drain_empty_socket() {
    let mut listener = GpuUeventListener::setup().expect("listener socket");

    // Leerer Socket: Drain liefert Ok(vec![]) — kein EAGAIN-Fehler.
    let events = listener.read_events().expect("read_events ok");
    assert!(events.is_empty(), "leerer Socket muss leer drainen");

    // Zweiter Aufruf: stabil (kein Zustand bleibt haengen)
    let events = listener.read_events().expect("read_events ok (2)");
    assert!(events.is_empty());
    eprintln!("smoke: setup + drain ok");
}
