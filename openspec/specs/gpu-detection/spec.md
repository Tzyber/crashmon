# Capability: GPU-Detection

Erkennung von GPU-Hangs, Driver-Resets und terminalen Wedged-Zustaenden fuer AMD (amdgpu), NVIDIA und Intel (i915/xe) — ueber Journal-Muster + Kernel-Uevents.

## Requirements

### GD-1: Xid-Fehlercodes erkennen
- **Entities:** EventKind::GpuXid
- **Priority:** P0
- **Enforced:** false
- **Test:** `tests/matcher_test.rs` (Xid-Fixtures 13/31/43/45/62/79)

Regex `NVRM: Xid (PCI:...): <code>`; Xid-Severity-Tabelle zentral in `gpu/matcher.rs`.

### GD-2: amdgpu-Reset-Sequenzen erkennen
- **Entities:** EventKind::GpuReset
- **Priority:** P0
- **Enforced:** false
- **Test:** `tests/matcher_test.rs`

`GPU reset begin!`, `ring ... timeout`, `GPU reset(n) succeeded!/failed` — vendor=`amdgpu`.

### GD-3: Intel-Hang/Wedge erkennen
- **Entities:** EventKind::GpuReset, EventKind::GpuWedged
- **Priority:** P0
- **Enforced:** false
- **Test:** `tests/matcher_test.rs`

`Resetting chip after gpu hang`, `GPU HANG`, `Device wedged`, `declared device ... as wedged`.

### GD-4: Wedged-Uevent ueber Netlink empfangen
- **Entities:** GpuUeventListener
- **Priority:** P1
- **Enforced:** false
- **Test:** Uevent-Parser-Test (Fixture-Buffer `WEDGED=bus-reset`)

`NETLINK_KOBJECT_UEVENT` (Gruppe 1), Filter `SUBSYSTEM=drm` + `WEDGED=`; Drain-Loop bis EAGAIN, `SO_RCVBUF` erhoeht.

## Invariants

### GD-INV-1: Kein Polling-Hotpath
- **Enforced:** false
- **Test:** Code-Review

Journal-FD + Netlink-Socket = event-getrieben. Kein Timer fuer Event-Quellen; RAS-Zaehler-Tick ist bewusst gestrichen (sysfs_notify nicht garantiert, Datei-Modes teils root-only).

### GD-INV-2: Defensiv ohne GPU
- **Enforced:** false
- **Test:** VM-/Container-Test

Fehlende GPU-Knoten/Pfade: Daemon laeuft weiter, nur `tracing::warn`, kein Absturz.

### GD-INV-3: Kein Hotplug-Scope
- **Enforced:** false
- **Test:** Code-Review

Kein ACTION=add/remove-Monitoring, kein `Hotplug`-Event — ausserhalb des Ziels (YAGNI). Nur `WEDGED=` (+ `ACTION=change` als Kontext).

### GD-INV-4: Vendor ohne Doppelpunkt (B2-Fix)
- **Enforced:** false
- **Test:** `tests/fixtures/kernel_lines.txt` (echte Zeilen aus Bugreports)

Vendor-Match ueber den Treiber-Namen (`amdgpu`, `i915`, `xe `), NICHT ueber
`amdgpu:` — die echte Kernelzeile ist `[drm:amdgpu_job_timedout [amdgpu]]
*ERROR* ring ... timeout`. Ring-Timeouts (inkl. soft-recovered) werden als
GpuReset erkannt. Fixtures muessen belegbare Herkunft haben.

### GD-INV-5: Wedged nur mit Treiber-Kontext (k7) + Kernel-Absender (W3)
- **Enforced:** false
- **Test:** `tests/matcher_test.rs` (k7-Negativfall), Code-Review uevent.rs (W3)

`wedged`-Match verlangt drm/xe/amdgpu/i915-Kontext und schneidet die
PCI-ID des Devices mit. Netlink-Uevents: `recvmsg` prueft den Absender
(`nl_pid == 0` und Multicast-Gruppe) — Unicast von lokalen Prozessen wird
verworfen (Spoofing-Guard wie libudev).
