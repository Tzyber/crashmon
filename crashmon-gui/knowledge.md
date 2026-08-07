# crashmon Wissensspeicher

Eigene Notizen zu Fehlern und neuen Dingen — wird in der GUI angezeigt.
Erweitere diese Datei mit deinem Editor; die GUI aktualisiert beim naechsten Poll.
(Repo-Datei: wird beim Bauen eingebettet und bei Erststart nach
`~/.local/share/crashmon/knowledge.md` kopiert — dort sammeln sich deine
Eintraege + Auto-gelerntes. Diese Vorlage hier ist der versionierbare Stand.)

## Xid-Codes (NVIDIA, verifiziert aus recherche-phase1.md)
- 13: hoch — Graphics Engine Exception (GPU-Engine-Fehler)
- 31: hoch — Illegal memory access (haeufigste Xid-Ursache)
- 43: hoch — GPU stopped processing (oft Folgexid nach 31)
- 45: hoch — Preemptive cleanup (oft Folgexid)
- 62: kritisch — Internal micro-controller halt (oft Hardware-Defekt)
- 79: fatal — GPU has fallen off the bus (PCIe/Hardware/Strom)

## Weitere Xid-Codes (NVIDIA, User-Ergaenzung)
- 8: mittel — FIFO error / Channel Command Error (Treiber-Haenger)
- 32: hoch — Invalid Context (Proton/Vulkan/DXVK Kontext-Verlust)
- 48: kritisch — Double Bit ECC Error (Hardware-RAM-Fehler auf VRAM)
- 92: hoch — High Temperature / Thermal Event (GPU schuetzt sich vor Ueberhitzung)
- 109: mittel — CTX Switch Timeout (DXVK/VKD3D Timeout bei Spiel-Szenenwechsel)

## Linux-Signale & Exit-Codes (Coredump-Ursachen)
- `SIGSEGV` (11 / Exit 139): Segmentation Fault — Ungueltiger Speicherzugriff (Nullpointer, Use-after-free, Buffer Overflow)
- `SIGABRT` (6 / Exit 134): Abort — Programm hat sich selbst beendet (assert() fehlgeschlagen, unbehandelte Exception)
- `SIGILL` (4 / Exit 132): Illegal Instruction — Ungueltiger CPU-Befehl (z. B. AVX-512-Binaries auf aelterer CPU, Stack-Corruption)
- `SIGFPE` (8 / Exit 136): Floating Point Exception — Division durch 0 oder Arithmetik-Fehler
- `SIGBUS` (7 / Exit 135): Bus Error — Ausrichtungsfehler beim Speicherzugriff oder mmap-Datei wurde untergepfluegt
- `SIGKILL` (9 / Exit 137) / `SIGTERM` (15 / Exit 143): Extern beendet — Prozess wurde vom User (kill) oder System geplaettet

## Gaming / Proton / Vulkan
- `radv: GPU reset` — AMD Vulkan (Mesa RADV) Treiber-Reset
- `vkd3d: Out of memory` — DirectX-12-zu-Vulkan-Uebersetzung ist vollgelaufen
- `DXVK: Device lost` — Vulkan-Geraet waehrend des Renderns verloren gegangen
- `wine: Unhandled page fault` — Windows-Binary in Wine/Proton abgestuerzt

## App-Panics (Rust / C++)
- `thread '...' panicked at ...` — Rust Unhandled Panic (fuehrt meist zu SIGABRT/Exit 101)
- `fatal runtime error: stack overflow` — Unendliche Rekursion / Stack voll
- `double free or corruption` — C/C++ Heap-Memory Corruption

## coredumpctl Quick-Commands
- Letzten Crash anzeigen: `coredumpctl info`
- Stacktrace ohne GDB direkt ausgeben: `coredumpctl info <pid>`
- Direkt in GDB springen: `coredumpctl debug <pid>`
- Alle Crashes einer Binary auflisten: `coredumpctl list <exe-name>`

## Typische Kernel-/Treiber-Meldungen
- `amdgpu: GPU reset begin!` — AMD-Treiber setzt GPU zurueck (hang recovery)
- `amdgpu: ring <name> timeout` — Engine/Queue haengt; folgt meist ein Reset
- `i915: Resetting chip after gpu hang` — Intel i915-Reset
- `GPU HANG: eGPU ...` — i915-Hang-Meldung
- `xe ... declared device ... as wedged` — Intel xe: GPU als dauerhaft haengend eingestuft
- `Out of memory: Killed process <pid> (<comm>)` — Kernel-OOM-Killer (global)
- `Memory cgroup out of memory: Killed process ...` — OOM in einer Cgroup (Container/User-Slice)
- `oom_reaper: reaped process ...` — OOM-Nachfolge (Speicher wurde zurueckgeholt)

## Troubleshooting
- Coredump analysieren: `gdb <exe> <core-datei>` (CORE-Pfad im Report)
- GPU-Resets beobachten: `dmesg | grep -i reset` / `journalctl -k | grep -i reset`
- Xids im Journal: `journalctl _TRANSPORT=kernel | grep -i xid`
- OOM-Kontext: `free -h`, `zramctl`, `systemd-analyze`; Cgroup-OOM: `journalctl | grep -i oom`
- NVIDIA-Treiberversion pruefen: `nvidia-smi`; Treiber-Log: `/var/log/Xorg.0.log`
- Wiederholte GPU-Resets: erst Treiber-Update, dann Hardware (Strom/PCIe-Slot) pruefen
