# Capability: Aggregation

Korrelation mehrerer Roh-Events zu einem Crash-Report. Dedupe, T0-Prinzip, Backpressure.

## Requirements

### AG-1: Xid-Bursts nach T0-Prinzip korrelieren
- **Entities:** CrashEvent
- **Priority:** P0
- **Enforced:** false
- **Test:** `tests/aggregate_test.rs` (Burst 31->43->45 = 1 Event, Ursache=Xid 31)

Erster Xid im Zeitfenster ist Ursache; Folge-Xids werden beigeordnet, nicht als eigene Events gemeldet.

### AG-2: PID + Zeitfenster korrelieren
- **Entities:** CrashEvent
- **Priority:** P0
- **Enforced:** false
- **Test:** `tests/aggregate_test.rs`

Coredump + Xid mit uebereinstimmender PID innerhalb 5 s = ein Report.

### AG-3: Bounded Channel mit expliziter Ueberlauf-Policy
- **Entities:** mpsc::channel(1024)
- **Priority:** P0
- **Enforced:** false
- **Test:** Ueberlauf-Test (Kanal voll -> drop-newest + Lost-Zaehler)

Unbounded = RAM-Gefahr; bounded+block = Stau am level-getriggerten FD. Lost-Zaehler im Report-Feld.

## Invariants

### AG-INV-1: Sortierung nach Event-Timestamp
- **Enforced:** false
- **Test:** `tests/aggregate_test.rs`

Ordnung und Fenster basieren auf `ts`, nie auf Empfangsreihenfolge (Backpressure).

### AG-INV-2: Kein RAM-Wachstum im Burst
- **Enforced:** false
- **Test:** Burst-Lasttest (bounded Channel, 0-Wachstum)

### AG-INV-3: Report erst nach Fenster-Schliessung
- **Enforced:** false
- **Test:** `tests/aggregate_test.rs`

Kein Report mit halber Korrelation (Fenster abwarten, dann emittieren).
