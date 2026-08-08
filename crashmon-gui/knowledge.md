# crashmon Wissensspeicher

Eigene Notizen zu Fehlern und neuen Dingen wird in der GUI angezeigt.
Erweitere diese Datei mit deinem Editor; die GUI aktualisiert beim naechsten Poll.
(Repo-Datei: wird beim Bauen eingebettet und bei Erststart nach
`~/.local/share/crashmon/knowledge.md` kopiert — dort sammeln sich deine
Eintraege + Auto-gelerntes. Diese Vorlage hier ist der versionierbare Stand.)

## Quellenmarken

Jeder Eintrag traegt eine Marke. Wer etwas ergaenzt, setzt sie mit.

- `[doc]` steht so in der Hersteller- oder Kernel-Dokumentation
- `[obs]` selbst beobachtet, Meldung im eigenen Journal gesehen
- `[?]` aus Foren oder Sekundaerquellen, nicht gegengeprueft

`[?]`-Eintraege sind ausdruecklich erlaubt, aber sie bleiben markiert, bis
jemand sie belegt. Lieber eine markierte Vermutung als eine unmarkierte.

---

## Xid-Codes (NVIDIA)

Bezeichnungen nach NVIDIA "XID Errors", Kapitel "Common Xid Errors".
Die Nummer ist kein Befund, sondern ein Wegweiser in eine Fehlerfamilie:
Anwendung, GPU-Speicher, Interconnect, Bus oder Treiber.

- 13: hoch, GR: SW Notify Error. Fehler in der Graphics Engine, meist von
  der Anwendung ausgeloest (Shader, Treiber-Bug) `[doc]`
- 31: hoch, FIFO: MMU Error. Ungueltiger Speicherzugriff, der haeufigste
  Xid ueberhaupt. Meist Software, nicht Hardware `[doc]`
- 32: hoch, PBDMA Error. Fehler im Pushbuffer-DMA-Strom `[doc]`
  (nicht "Invalid Context", das ist eine verbreitete Forenbezeichnung)
- 43: hoch, Reset Channel Verif Error. Der Kanal wurde zurueckgesetzt,
  oft Folgexid nach 31 `[doc]`
- 45: hoch, OS: Preemptive Channel Removal. Meist Folgexid, selten Ursache `[doc]`
- 48: kritisch, DBE (Double Bit Error) ECC. Echter Hardware-Speicherfehler
  im VRAM. Einmalig kann Zufall sein, wiederholt ist es Degradation `[doc]`
- 62: kritisch, Internal micro-controller halt `[doc]`
- 63, 64: ECC Page Retirement bzw. Row Remapping. 63 = die Selbstheilung
  hat gegriffen (Reset noetig, damit sie wirkt), 64 = sie ist
  fehlgeschlagen `[doc]`
- 74: NVLink Error, nur auf Systemen mit NVLink/NVSwitch relevant `[doc]`
- 79: fatal, GPU has fallen off the bus. Die Karte ist ueber PCIe nicht
  mehr erreichbar. Reihenfolge beim Suchen: Strom, Riser/Slot, dann Karte `[doc]`
- 92: hoch, hohe Single-Bit-ECC-Fehlerrate. **Speicherfehler, nicht
  Temperatur.** NVIDIA fuehrt 48, 63, 64, 92, 94 und 95 gemeinsam als
  Speicherfehler-Familie `[doc]`
- 93: Non-fatal violation of provisioned InfoROM wear limit `[doc]`
- 94, 95: Contained bzw. uncontained ECC error. Bei 94 stoppt die
  betroffene Anwendung, bei 95 ist der Fehler nicht mehr eingegrenzt `[doc]`
  (Achtung: manche Sekundaerquellen beschreiben 94 als rein informativ,
  das widerspricht der Herstellerdoku)
- 110: Security fault error `[doc]`
- 119, 120: GSP RPC Timeout bzw. GSP Error. Firmware-Ebene, oft mit
  Treiberversion verknuepft `[doc]`
- 8: unklar. Taucht in Sekundaerlisten als "FIFO error" auf, steht aber
  nicht in der NVIDIA-Kapitelliste. Wer eine echte Quelle hat: bitte
  nachtragen `[?]`
- 109: CTX Switch Timeout, verbreitete Bezeichnung, in der offiziellen
  Kapitelliste nicht enthalten `[?]`

Xids lesen: `journalctl -k -g "NVRM: Xid"` oder `dmesg | grep -i xid`.
Ein Xid-Burst innerhalb weniger Sekunden ist ein Ereignis, nicht zehn.
crashmon fasst das nach dem T0-Prinzip zusammen.

## AMD (amdgpu)

- `amdgpu: GPU reset begin!` gefolgt von `GPU reset succeeded`: der
  Treiber hat einen Hang erkannt und die GPU zurueckgesetzt. Ein
  einzelner Reset unter Last ist unschoen, aber ueberlebbar `[obs]`
- `amdgpu: ring gfx_0.0.0 timeout, signaled seq=..., emitted seq=...`:
  eine Engine antwortet nicht mehr. Steht praktisch immer direkt vor
  einem Reset. Der Ringname sagt, welche Engine (gfx, sdma, comp, vcn) `[obs]`
- `[drm] *ERROR* [CRTC:...] flip_done timed out` /
  `amdgpu_dm ... Waiting for fences timed out`: Display-Pipeline haengt.
  Typisch bei Monitorwechsel, Aufloesungswechsel oder VRR-Problemen `[obs]`
- `amdgpu: [gfxhub] page fault (src_id:0 ring:...)` mit
  `VM_L2_PROTECTION_FAULT_STATUS`: die GPU hat auf nicht gemappten
  Speicher zugegriffen. Das AMD-Gegenstueck zu Xid 31, meist ein Bug im
  Spiel, in Mesa oder in DXVK, selten Hardware `[obs]`
- `amdgpu: MES failed to respond to msg`: der Microengine-Scheduler
  reagiert nicht, auf RDNA3/4 firmwarenah `[?]`
- `drm:amdgpu_job_timedout`: der Job-Timeout, der den Reset ausloest `[obs]`

Nuetzlich:
- Resets zaehlen: `journalctl -k -g "GPU reset"`
- Treiber-/Firmware-Stand: `journalctl -k -b -g amdgpu | head -40`
- Recovery abschalten zum Debuggen (dann friert es statt sich zu erholen):
  `amdgpu.gpu_recovery=0` als Kernelparameter
- Mesa/RADV-Workarounds gehen ueber `RADV_DEBUG`, z. B.
  `RADV_DEBUG=nohiz,nodcc` gegen Darstellungsfehler mit DCC `[obs]`

## Intel

- `i915: Resetting chip after gpu hang` / `GPU HANG:`: i915-Reset `[obs]`
- `xe ... declared device ... as wedged`: der neuere xe-Treiber stuft die
  GPU als dauerhaft haengend ein. Anders als ein Reset ist das ein
  Endzustand, es hilft nur ein Neustart `[doc]`

## Linux-Signale und Exit-Codes

Ein von einem Signal beendeter Prozess meldet der Shell `128 + Signalnummer`.
Deshalb 139 fuer SIGSEGV (128 + 11). Ein Exit-Code unter 128 kommt vom
Programm selbst, nicht vom Kernel.

- `SIGSEGV` (11 / Exit 139): Segmentation Fault, ungueltiger Speicherzugriff
  (Nullpointer, Use-after-free, Buffer Overflow)
- `SIGABRT` (6 / Exit 134): das Programm hat sich selbst beendet,
  fehlgeschlagenes `assert()`, unbehandelte C++-Exception, Rust-Panic mit
  abort, glibc-Meldung wie `double free or corruption`
- `SIGILL` (4 / Exit 132): ungueltiger CPU-Befehl. Binary nutzt
  Instruktionen, die die CPU nicht hat (AVX-512 auf aelterer CPU), oder
  der Codepfad wurde zerstoert
- `SIGFPE` (8 / Exit 136): Arithmetikfehler, Division durch 0
- `SIGBUS` (7 / Exit 135): Ausrichtungsfehler oder eine gemappte Datei ist
  unter dem Prozess weggerutscht. Auf dem Desktop fast immer der zweite
  Fall: ein AppImage oder eine .so wurde waehrend der Ausfuehrung ersetzt
  oder abgeschnitten, oder `/tmp` (tmpfs) ist vollgelaufen `[obs]`
- `SIGKILL` (9 / Exit 137): von aussen hart beendet. Erzeugt **keinen**
  Coredump. Kommt der Kill vom Kernel-OOM-Killer, steht die Begruendung
  nur im Journal, nicht beim Prozess
- `SIGTERM` (15 / Exit 143): hoefliche Aufforderung zu beenden, regulaerer
  Shutdown-Weg

## Out of Memory

Es gibt drei verschiedene Killer, die unterschiedlich melden. Wer nur nach
"Out of memory" sucht, findet die anderen beiden nicht.

- `Out of memory: Killed process <pid> (<comm>) total-vm:...kB,
  anon-rss:...kB`: der Kernel-OOM-Killer, global. `anon-rss` ist der real
  belegte Speicher, `total-vm` fast immer irrefuehrend gross `[doc]`
- `Memory cgroup out of memory: Killed process ...`: das Limit einer
  Cgroup war erreicht, nicht der Systemspeicher. Typisch in Containern
  und in User-Slices `[doc]`
- `systemd-oomd: Killed ...` bzw. `systemd-oomd` im Unit-Kontext: der
  Userspace-Killer greift **vor** dem Kernel ein, anhand von
  Memory-Pressure (PSI). Er beendet ganze Slices, nicht einzelne
  Prozesse. Konfiguration in `oomd.conf` und den Unit-Dateien `[doc]`
- `earlyoom` macht dasselbe auf anderem Weg und meldet eigenstaendig `[?]`
- `oom_reaper: reaped process ...`: Nachbereitung, der Speicher wurde
  zurueckgeholt. Kein eigener Vorfall

Kontext einsammeln: `free -h`, `zramctl` (auf komprimiertem Swap sieht
"voll" anders aus), `systemd-cgtop`, und
`journalctl -k -b -g "Out of memory"`.

## Hardware jenseits der GPU

- `mce: [Hardware Error]: Machine Check Exception`: CPU oder Speicher.
  Mit `rasdaemon` oder `mcelog` dekodierbar, roh ist die Meldung kaum
  lesbar `[doc]`
- `EDAC MC0: ... CE memory read error`: korrigierter RAM-Fehler. Einzeln
  harmlos, in Serie ein Riegel auf dem Weg nach draussen `[doc]`
- `watchdog: BUG: soft lockup - CPU#3 stuck for 22s!`: ein Kern haengt in
  einer Kernelschleife. Meist Treiber, oft der letzte Eintrag vor einem
  Freeze `[doc]`
- `pcieport 0000:00:01.0: AER: Corrected error received`: PCIe-Fehler.
  Einzeln normal, gehaeuft vor GPU-Verlusten (siehe Xid 79) ein starkes
  Signal `[obs]`
- `nvme nvme0: I/O <n> QID <n> timeout, aborting` /
  `blk_update_request: I/O error`: der Datentraeger antwortet nicht.
  Erklaert auch Abstuerze, die wie Softwarefehler aussehen `[obs]`
- `BTRFS error (device ...): parent transid verify failed`: Btrfs-Metadaten
  passen nicht zusammen. Auf CachyOS die Standard-Dateisystemmeldung, die
  man kennen sollte. Nicht ignorieren `[obs]`
- `EXT4-fs error (device ...)`: das ext4-Gegenstueck `[doc]`
- Kernel-Taint pruefen: `cat /proc/sys/kernel/tainted`, im Log als
  `Tainted: P W O`. Proprietaere Module (P), vorheriger Warn (W), externes
  Modul (O). Bei Bugreports die erste Frage `[doc]`

## Gaming, Proton, Vulkan

- `radv: GPU reset`: der Mesa-RADV-Treiber hat einen Reset gesehen
- `vkd3d: Out of memory`: die DirectX-12-nach-Vulkan-Uebersetzung ist
  vollgelaufen, oft VRAM statt RAM
- `DXVK: Device lost`: das Vulkan-Geraet ging waehrend des Renderns
  verloren. Praktisch immer die Folge eines GPU-Resets, also die Ursache
  eine Ebene tiefer suchen `[obs]`
- `wine: Unhandled page fault on read access to ...`: das Windows-Binary
  ist abgestuerzt, nicht Wine selbst
- Proton-Log einschalten: `PROTON_LOG=1 %command%` in den Startoptionen,
  Ergebnis in `~/steam-<appid>.log` `[obs]`
- Ein Absturz mit gleichzeitigem `ring ... timeout` im Kernel ist ein
  GPU-Problem, ein Absturz ohne Kernelmeldung ist ein Spielbug

## App-Panics (Rust, C++)

- `thread '...' panicked at ...`: Rust-Panic. Mit `panic=unwind` (Default)
  meist Exit 101 ohne Coredump, mit `panic=abort` SIGABRT mit Coredump
- `fatal runtime error: stack overflow`: unendliche Rekursion. Rust faengt
  das ueber die Guard Page ab und meldet es sauber, C stuerzt einfach ab
- `double free or corruption` / `malloc(): invalid size`: glibc hat
  Heap-Korruption bemerkt und bricht mit SIGABRT ab. Die Meldung nennt den
  Ort des Auffallens, nicht den Ort des Fehlers
- `RUST_BACKTRACE=1` setzen, bevor man einen Rust-Absturz reproduziert

## Coredumps praktisch

- Letzte Abstuerze auflisten: `coredumpctl list`
- Details plus Stacktrace: `coredumpctl info <pid|exe>`
- In den Debugger: `coredumpctl debug <pid>`
- Core-Datei herausholen: `coredumpctl dump <pid> > /tmp/core`
  (systemd legt sie zstd-komprimiert ab, deshalb nicht einfach kopieren)
- Manuell: `gdb <exe> <core-datei>`, der Pfad steht im crashmon-Report

**Warum die Core-Datei fehlt, obwohl der Report sie nennt:** systemd raeumt
den Coredump-Speicher selbststaendig auf. Die Grenzen stehen in
`/etc/systemd/coredump.conf` (`MaxUse`, `KeepFree`, `ProcessSizeMax`,
`ExternalSizeMax`). Grosse Prozesse werden gar nicht erst vollstaendig
gespeichert, alte Dumps verschwinden nach Platzbedarf. Der Journal-Eintrag
bleibt, die Datei nicht. `[doc]`

**Wenn gar kein Coredump entsteht:**
- `cat /proc/sys/kernel/core_pattern` muss auf systemd-coredump zeigen
- `ulimit -c` darf nicht 0 sein (in der Shell, aus der gestartet wurde)
- SIGKILL erzeugt grundsaetzlich keinen Dump
- setuid-Programme dumpen nur bei gesetztem `fs.suid_dumpable`

## Journal lesen

- Nur Kernel, aktueller Boot: `journalctl -k -b`
- **Voriger Boot: `journalctl -k -b -1`.** Nach einem harten Freeze oder
  Reset ist das der einzige Weg an die Meldungen, im aktuellen Boot steht
  nichts davon
- Welche Boots gibt es: `journalctl --list-boots`
- Nur Fehler: `journalctl -b -p err`
- Mustersuche im Journal statt mit grep: `journalctl -k -g "reset|timeout"`
- Zeitfenster um einen Absturz: `journalctl --since "16:30" --until "16:40"`
- Live mitlesen: `journalctl -kf`
- Ein Coredump-Ereignis vollstaendig: `journalctl -t systemd-coredump -o verbose`

Wenn das Journal nach einem Absturz leer ist: es war ein harter Hang, bei
dem nichts mehr auf die Platte geschrieben wurde. Dann bleibt `-b -1`,
und wenn dort auch nichts steht, hilft nur ein serielles oder
Netconsole-Log.

## Vorgehen bei wiederholten GPU-Resets

1. Kommt es nur in einem Spiel oder in einer Anwendung? Dann ist es
   zuerst deren Bug, nicht die Karte.
2. Treiber- und Mesa-Version notieren, dann eine Version zurueck oder vor
   testen. Regressionen sind haeufiger als Defekte.
3. Kernel-Log um den Reset vollstaendig lesen, nicht nur die Resetzeile:
   die Ursache steht meist zwei bis zehn Zeilen davor.
4. Erst dann Hardware: Netzteil und Kabel, PCIe-Slot, Undervolting oder
   OC zuruecknehmen, Temperaturen unter Last mitschreiben.
5. Gehaeufte AER-Meldungen von `pcieport` vor dem Reset verschieben den
   Verdacht deutlich in Richtung Verbindung oder Stromversorgung.
