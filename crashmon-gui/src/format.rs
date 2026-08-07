//! Formatierung: Zeitstempel (µs UTC) + Report-Kurzfassungen.
//!
//! `format_ts` hand-rolled (Hinnant civil_from_days) statt chrono —
//! Minimal-Deps-Philosophie des Projekts; komplett unit-getestet.

use crash_daemon::event::EventKind;
use crash_daemon::output::Report;

/// µs seit Epoch -> "YYYY-MM-DD HH:MM:SS.mmm UTC" (Hinnant-Algorithmus).
pub fn format_ts(ts_us: u64) -> String {
    let secs = ts_us / 1_000_000;
    let days = (secs / 86_400) as i64;
    let tod = secs % 86_400;
    let (hh, mm, ss) = (tod / 3600, (tod % 3600) / 60, tod % 60);

    // civil_from_days (Howard Hinnant, date.h): days -> (y, m, d)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03} UTC",
        y,
        m,
        d,
        hh,
        mm,
        ss,
        (ts_us % 1_000_000) / 1_000
    )
}

/// Relative Zeit (D3): "gerade eben", "vor 2 Min", "vor 3 Std", "vor 5 Tagen"
/// — die Liste soll nicht mit vollen UTC-Stempeln konkurrieren.
pub fn relative_ago(ts_us: u64, now_us: u64) -> String {
    let diff = now_us.saturating_sub(ts_us) / 1_000_000; // Sekunden
    if diff < 10 {
        "gerade eben".into()
    } else if diff < 60 {
        format!("vor {diff} Sek")
    } else if diff < 3600 {
        format!("vor {} Min", diff / 60)
    } else if diff < 86_400 {
        format!("vor {} Std", diff / 3600)
    } else {
        format!("vor {} Tagen", diff / 86_400)
    }
}

/// Lokalzeit (D3): UTC im Report-JSON ist richtig, UTC in der Oberflaeche
/// ist Zumutung — Offset aus libc::localtime_r (kein chrono noetig).
pub fn format_ts_local(ts_us: u64) -> String {
    let secs = (ts_us / 1_000_000) as libc::time_t;
    // SAFETY: localtime_r schreibt in unser zeroed tm, kein Globals-Zustand.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let ret = unsafe { libc::localtime_r(&secs, &mut tm) };
    if ret.is_null() {
        return format_ts(ts_us); // Fallback: UTC-Formatierung
    }
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}

/// Kurzzeile fuer die Report-Liste, z. B. "Coredump SIGSEGV: app (pid 42)".
pub fn summarize(report: &Report) -> String {
    let detail = match &report.cause.kind {
        EventKind::Coredump {
            pid, comm, signal, ..
        } => match signal {
            Some(s) => format!("{s}: {comm} (pid {pid})"),
            None => format!("{comm} (pid {pid})"),
        },
        EventKind::OomKill { pid, comm } => format!("{comm} (pid {pid})"),
        EventKind::GpuXid { code, pci, .. } => match pci {
            Some(p) => format!("Xid {code} ({p})"),
            None => format!("Xid {code}"),
        },
        EventKind::GpuReset { vendor, .. } => format!("GPU Reset ({vendor})"),
        EventKind::GpuWedged { method, .. } => match method {
            Some(m) => format!("GPU Wedged ({m})"),
            None => "GPU Wedged".into(),
        },
    };
    match &report.cause.kind {
        EventKind::Coredump { .. } => format!("Coredump {detail}"),
        EventKind::OomKill { .. } => format!("OOM-Kill {detail}"),
        EventKind::GpuXid { .. } => format!("NVIDIA {detail}"),
        EventKind::GpuReset { .. } => detail,
        EventKind::GpuWedged { .. } => detail,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crash_daemon::event::{CrashEvent, EventKind};

    fn report_with(kind: EventKind) -> Report {
        Report {
            ts: 1_000_000_000_000_000,
            cause: CrashEvent {
                ts: 1_000_000_000_000_000,
                kind,
            },
            related: vec![],
            lost_events: 0,
        }
    }

    #[test]
    fn epoch_zero() {
        assert_eq!(format_ts(0), "1970-01-01 00:00:00.000 UTC");
    }

    #[test]
    fn y2k() {
        // 2000-01-01 00:00:00 UTC = 946684800 s
        assert_eq!(
            format_ts(946_684_800_000_000),
            "2000-01-01 00:00:00.000 UTC"
        );
    }

    #[test]
    fn y2038_fixpunkt() {
        // 2038-01-19 03:14:07 UTC = 2147483647 s (32-bit-Overflow-Fixpunkt)
        assert_eq!(
            format_ts(2_147_483_647_000_000),
            "2038-01-19 03:14:07.000 UTC"
        );
    }

    #[test]
    fn millis_anteil() {
        assert_eq!(
            format_ts(1_786_126_270_648_827),
            "2026-08-07 18:11:10.648 UTC"
        );
    }

    #[test]
    fn summarize_coredump() {
        let r = report_with(EventKind::Coredump {
            pid: 42,
            exe: None,
            comm: "firefox".into(),
            signal: Some("SIGSEGV".into()),
            uid: None,
            unit: None,
            coredump_file: None,
        });
        assert_eq!(summarize(&r), "Coredump SIGSEGV: firefox (pid 42)");
    }

    #[test]
    fn summarize_xid() {
        let r = report_with(EventKind::GpuXid {
            code: 31,
            pci: Some("0000:03:00".into()),
            pid: None,
            message: "x".into(),
        });
        assert_eq!(summarize(&r), "NVIDIA Xid 31 (0000:03:00)");
    }

    #[test]
    fn summarize_oom() {
        let r = report_with(EventKind::OomKill {
            pid: 7,
            comm: "app".into(),
        });
        assert_eq!(summarize(&r), "OOM-Kill app (pid 7)");
    }

    #[test]
    fn relative_ago_steps() {
        let now = 1_000_000_000_000_000u64; // willkuerliche Basis
        assert_eq!(relative_ago(now, now), "gerade eben");
        assert_eq!(relative_ago(now - 5_000_000, now), "gerade eben");
        assert_eq!(relative_ago(now - 30_000_000, now), "vor 30 Sek");
        assert_eq!(relative_ago(now - 120_000_000, now), "vor 2 Min");
        assert_eq!(relative_ago(now - 3 * 3600_000_000, now), "vor 3 Std");
        assert_eq!(relative_ago(now - 5 * 86_400_000_000, now), "vor 5 Tagen");
        // Zukunft (Uhr driftet): saturating -> "gerade eben"
        assert_eq!(relative_ago(now + 60_000_000, now), "gerade eben");
    }

    #[test]
    fn format_ts_local_matches_utc_when_zone_is_utc() {
        // In UTC-Umgebung sind local und utc identisch; das Format ist
        // "YYYY-MM-DD HH:MM:SS" ohne UTC-Suffix (D3: Lokalzeit).
        let s = format_ts_local(946_684_800_000_000); // 2000-01-01 00:00:00 UTC
        assert_eq!(s.len(), 19, "Format: {s}");
        assert!(s.starts_with("2000-01-01 "), "{s}");
        assert!(!s.contains("UTC"), "kein UTC-Suffix: {s}");
    }
}
