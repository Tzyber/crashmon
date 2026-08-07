# Capability: Output

Persistenz der Crash-Reports als strukturierte JSON-Dateien.

## Requirements

### OU-1: Atomische JSON-Reports
- **Entities:** `crash-<ts>.json`
- **Priority:** P0
- **Enforced:** false
- **Test:** `tests/output_test.rs` (temp+rename, kein Halbzustand)

Schreiben via temp-Datei + `rename()` in `--dump-dir`; `ts` = UTC-µs im Dateinamen und Report.

### OU-2: Vollstaendiger Report-Inhalt
- **Entities:** CrashReport
- **Priority:** P0
- **Enforced:** false
- **Test:** `tests/output_test.rs`

Felder: Event-Kind, ts, exe/comm/pid/signal (Coredump), Xid-Code/PID-Korrelation (GPU), Lost-Events-Zaehler.

## Invariants

### OU-INV-1: UTC-µs-Zeitstempel, nie Lokalzeit
- **Enforced:** false
- **Test:** `tests/output_test.rs`

### OU-INV-2: Schreibbar unter ProtectSystem=strict
- **Enforced:** false
- **Test:** systemd-Unit-Test

`StateDirectory=crashmon` + `ReadWritePaths` — Dump-Verzeichnis beschreibbar trotz strikter Härtung.

### OU-INV-3: Kein Datenverlust bei Shutdown
- **Enforced:** false
- **Test:** Shutdown-Test

Laufende Reports werden vor Exit fertig geschrieben (Drain mit Timeout, `TimeoutStopSec=10`).
