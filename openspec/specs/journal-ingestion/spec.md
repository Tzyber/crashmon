# Capability: Journal-Ingestion

Ereignisquelle: systemd-Journal. Erfasst Coredumps (systemd-coredump), OOM-Killer-Events und Kernel-Treibermeldungen (GPU) — unprivilegiert, event-getrieben.

## Requirements

### JI-1: Coredump-Events erfassen
- **Entities:** CrashEvent, EventKind::Coredump
- **Priority:** P0
- **Enforced:** false
- **Test:** `tests/matcher_test.rs` (MESSAGE_ID-Fixture), Integration: Smoke-Read Host-Journal

Matcht `MESSAGE_ID=fc2e22bc6ee647b6b90729ab34a250b1` und liest `COREDUMP_PID`, `COREDUMP_EXE`, `COREDUMP_COMM`, `COREDUMP_SIGNAL_NAME`, `COREDUMP_UID`, `COREDUMP_UNIT`, `COREDUMP_FILENAME`.

### JI-2: Kernel-Events erfassen
- **Entities:** EventKind::OomKill, EventKind::GpuXid, EventKind::GpuReset, EventKind::GpuWedged
- **Priority:** P0
- **Enforced:** false
- **Test:** `tests/matcher_test.rs` (OOM-/Xid-/GPU-Fixtures)

Matcht `_TRANSPORT=kernel`. OOM-PID/COMM per Regex aus `MESSAGE` (`Killed process <pid> (<comm>)`, `task=<comm>,pid=<pid>`).

### JI-3: Event-getrieben ohne Polling
- **Entities:** JournalSource::wait_readable
- **Priority:** P0
- **Enforced:** false
- **Test:** `tests/journal_source_test.rs` (Fake treibt Warteschritt)

Reader parkt auf sd-journal-FD (epoll via `AsyncFd`); kein Timer-Polling.

### JI-4: Cursor-Persistenz ueber Neustarts
- **Entities:** State-Datei `/var/lib/crashmon/cursor`
- **Priority:** P0
- **Enforced:** false
- **Test:** Restart-Test (Events nicht doppelt, Downtime dokumentiert verpasst)

`get_cursor()`/`set_cursor()`; bei gueltigem Cursor Fortsetzung, sonst `seek(Tail)`.

## Invariants

### JI-INV-1: Keine Root-Rechte
- **Enforced:** false
- **Test:** systemd-Unit-Review + Smoke-Read als crashmon-User

Lesezugriff nur ueber Gruppe `systemd-journal`; kein Capability, kein Setuid.

### JI-INV-2: Zwei gematchte Handles, kein Soft-Filter-alles
- **Enforced:** false
- **Test:** Code-Review (`ingest/journal.rs`)

Handle A: MESSAGE_ID-Match. Handle B: `_TRANSPORT=kernel`-Match. Keine Deserialisierung+Regex ueber jeden Journal-Eintrag.

### JI-INV-3: Zeitsemantik
- **Enforced:** false
- **Test:** `tests/matcher_test.rs`

`ts` aus `_SOURCE_REALTIME_TIMESTAMP` (µs UTC-Epoch), Fallback Empfangszeit. Nie Aggregator-Empfangszeit als Event-Zeit.

### JI-INV-4: `Journal` bleibt in EINER Task
- **Enforced:** false
- **Test:** Compile-Time (LocalSet/current-thread) — `Journal` ist `!Send`

Kein Teilen ueber Task-Grenzen, kein FD-Herausloesen.

### JI-INV-5: Drain bis Exhausted (B1-Fix)
- **Enforced:** false
- **Test:** `journal.rs` next_event tri-state + Daemon-Drain-Loop (Review B1)

`next_event` liefert drei Zustaende: Event / BudgetSpent (Zeitscheibe voll,
`yield_now` + weiterlesen) / Exhausted (wirklich leer). Nicht-matchenden
Eintraege beenden den Drain NICHT — ein amdgpu-Hang (20–50 Kernelzeilen)
wird in einem Rutsch abgearbeitet statt mit ~2 Eintraegen pro Minute zu
troepfeln. Budget: 512 Eintraege pro Aufruf.
