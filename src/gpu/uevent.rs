//! Netlink-KOBJECT-Uevent-Listener.
//!
//! Quellen: recherche-phase1.md 2.4. `AF_NETLINK`/`NETLINK_KOBJECT_UEVENT`,
//! Gruppe 1 (unprivilegiert, kein Root noetig). Filter: `SUBSYSTEM=drm` UND
//! `WEDGED=` gesetzt (Linux ~6.14/6.15 meldet getriebeinduzierte Wedges dort;
//! rebind | bus-reset | reboot). Drain-Loop bis EAGAIN, `SO_RCVBUF`
//! hochgesetzt (Kernel klemmt unprivilegiert auf rmem_max und verdoppelt
//! intern; bei Puffer-Ueberlauf werden Uevents still verworfen — fuer
//! seltene WEDGED-Events unkritisch). Kein Hotplug-Monitoring (YAGNI,
//! Plan-Review). Senden an KOBJECT_UEVENT ist root-only — nur Empfang.
//!
//! Events haben kein eigenes Timestamp-Feld — `ts` ist Empfangszeit
//! (Plan-Design 3: Fallback Empfangszeit).

use crate::event::EventKind;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::time::{SystemTime, UNIX_EPOCH};

/// Puffergroesse fuer eine Uevent-Nachricht (typisch < 4 KiB).
const BUF_SIZE: usize = 128 * 1024;
/// SO_RCVBUF: hoher Puffer, um unter Bursts nichts zu verlieren.
const RCVBUF_SIZE: libc::c_int = 512 * 1024;

/// Parst einen Uevent-Buffer (`KEY=VALUE`\0-getrennt) auf WEDGED-Events.
/// `None` fuer alles andere (Hotplug, andere Subsysteme, Muell).
pub fn parse_uevent(buf: &[u8]) -> Option<EventKind> {
    let mut subsystem: Option<&[u8]> = None;
    let mut action: Option<&[u8]> = None;
    let mut wedged: Option<&[u8]> = None;

    for line in buf.split(|&b| b == 0) {
        if line.is_empty() {
            continue;
        }
        let (key, value) = match line.iter().position(|&b| b == b'=') {
            Some(pos) => (&line[..pos], &line[pos + 1..]),
            None => continue, // Zeilen ohne '=' ignorieren
        };
        match key {
            b"ACTION" => action = Some(value),
            b"SUBSYSTEM" => subsystem = Some(value),
            b"WEDGED" => wedged = Some(value),
            _ => {}
        }
    }

    if action != Some(b"change") || subsystem != Some(b"drm") {
        return None;
    }
    // WEDGED gesetzt (auch leer = Wedge ohne Methode)
    match wedged {
        Some(m) if !m.is_empty() => Some(EventKind::GpuWedged {
            method: Some(String::from_utf8_lossy(m).into_owned()),
        }),
        Some(_) => Some(EventKind::GpuWedged { method: None }),
        None => None,
    }
}

/// Nicht klonbarer Uevent-Socket (Raw-FD, laeuft auf der current_thread-Task).
pub struct GpuUeventListener {
    fd: OwnedFd,
}

impl GpuUeventListener {
    /// Legt den Netlink-Socket an (nonblocking, CLOEXEC, Gruppe 1, RCVBUF).
    pub fn setup() -> io::Result<Self> {
        // SAFETY: socket() ist ein reiner Syscall ohne Speicher-Invarianten.
        let fd = unsafe {
            libc::socket(
                libc::AF_NETLINK,
                libc::SOCK_RAW | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
                libc::NETLINK_KOBJECT_UEVENT,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: fd ist ab jetzt unser owned FD (SOCK_CLOEXEC verhindert
        // Leaks in Kindprozesse; OwnedFd schliesst ihn beim Drop).
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };

        // SAFETY: sockaddr_nl ist POD; zeroed() initialisiert alle Felder
        // (auch das private nl_pad) auf 0. bind() ist ein reiner Syscall.
        let mut addr: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
        addr.nl_family = libc::AF_NETLINK as libc::sa_family_t;
        // nl_pid = 0: Kernel-Autobind (kollisionsfrei, kein Unicast noetig —
        // wir empfangen nur Multicast ueber Gruppe 1; Senden ist eh root-only).
        addr.nl_pid = 0;
        addr.nl_groups = 1; // Gruppe 1 = KERNEL/KERNEL_UEVENT
        let ret = unsafe {
            libc::bind(
                fd.as_raw_fd(),
                &addr as *const libc::sockaddr_nl as *const libc::sockaddr,
                std::mem::size_of::<libc::sockaddr_nl>() as libc::socklen_t,
            )
        };
        if ret < 0 {
            return Err(io::Error::last_os_error());
        }

        // SO_RCVBUF hochsetzen (Kernel verdoppelt, wir bestellen genug).
        // SAFETY: fcntl/setsockopt ist ein reiner Syscall.
        let ret = unsafe {
            libc::setsockopt(
                fd.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_RCVBUF,
                &RCVBUF_SIZE as *const libc::c_int as *const libc::c_void,
                std::mem::size_of::<libc::c_int>() as libc::socklen_t,
            )
        };
        if ret < 0 {
            return Err(io::Error::last_os_error());
        }

        Ok(Self { fd })
    }

    /// Liest alle gerade verfuegbaren Uevents (Drain bis EAGAIN).
    /// Zurueck: geparste Wedge-Events mit Empfangszeit-`ts` (µs).
    ///
    /// Konsument (Phase 2.5): Polling via `tokio::time::sleep` (50–250 ms),
    /// kein AsyncFd — WEDGED-Erkennung ist nicht zeitkritisch (der Crash ist
    /// bereits passiert). Nach `Err` Socket neu aufsetzen mit Backoff.
    pub fn read_events(&mut self) -> io::Result<Vec<crate::event::CrashEvent>> {
        let mut events = Vec::new();
        // Buffer ueber den Drain heben: recv ueberschreibt eh, kein
        // 128-KiB-Memset pro Nachricht (Kernel-Uevents sind ~2 KiB).
        let mut buf = [0u8; BUF_SIZE];
        loop {
            // SAFETY: recv auf owned, nicht-blockierendem FD mit ausreichend
            // grossem Buffer — kein Speicher-Invarianten-Risiko.
            let n =
                unsafe { libc::recv(self.fd.as_raw_fd(), buf.as_mut_ptr() as *mut _, BUF_SIZE, 0) };
            if n < 0 {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::WouldBlock {
                    break; // Drain fertig
                }
                if err.kind() == io::ErrorKind::Interrupted {
                    continue; // Signal unterbrach den Syscall — Drain fortsetzen
                }
                return Err(err);
            }
            let Some(kind) = parse_uevent(&buf[..n as usize]) else {
                continue;
            };
            let ts = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_micros() as u64)
                .unwrap_or(0);
            events.push(crate::event::CrashEvent { ts, kind });
        }
        Ok(events)
    }
}
