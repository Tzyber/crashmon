# crashmon-GUI: Tray-Modus — Design-Spec

Datum: 2026-08-08
Status: abgestimmt (Brainstorming mit Review-Runde)

## Ziel

Die GUI läuft nach Fenster-Schließen als Tray-Icon weiter (StatusNotifierItem),
überwacht weiterhin den Report-Ordner und meldet neue Crashes. „Beenden" nur
explizit über das Tray-Menü. Kein Waisen-Daemon in keinem Pfad. Daemon startet
beim GUI-Start automatisch.

## Gemessene Grundlagen (Probe-Tests, nicht Annahmen)

1. **`logic()` läuft im Hidden-Zustand weiter.** Wegwerf-Probe (Close →
   CancelClose → Visible(false)): Ticks liefen 46 s nach Hide weiter, alle
   ~480 ms. Kein PollWorker nötig — `DaemonState`, `LogTail`, `known_ts`,
   `reports` bleiben im GUI-State, kein Thread-Umbau.
2. **Hidden-Zustand ist billig.** utime-Messung `/proc/self/stat`: sichtbar
   15–17 ticks/60 s, hidden 6 ticks/60 s (~0,1 % CPU). `request_repaint_after
   (500 ms)` und Poll-Intervall 500 ms bleiben unverändert.

## Architektur

- Einziger zusätzlicher Thread: der ksni-D-Bus-Thread (von ksni selbst).
- `ksni = { version = "=0.3.6", default-features = false, features =
  ["blocking", "async-io"] }` — kein tokio (Projekt-Philosophie: exakt
  gepinnt, tokio-frei).
- Tray-`Handle<T>` liegt im GUI-State; Menü-Callbacks (D-Bus-Thread) machen
  nur Sender-Push (`std::sync::mpsc`), keine GUI-Arbeit.
- `spawn()` beim GUI-Start. `Err(WontShow)` („no StatusNotifierHost exists",
  verifiziert in ksni-Source) → kein Tray-Modus: Fenster-X beendet die App
  wie bisher, Statuszeile „Tray nicht verfügbar — Fenster-X beendet die App".

## Daemon-Lifecycle

### Autostart + Single-Instance

- Daemon startet automatisch beim GUI-Start (User-Entscheidung Q1).
- **Vor `mount()`: Probe-Flock (`LOCK_EX|LOCK_NB`) auf `dump_dir/.lock`.**
  - Frei → Lock sofort wieder freigeben, Daemon normal spawnen (der Daemon
    selbst flockt danach — W4 `InstanceLock`, daemon.rs:48-89).
  - Belegt → **`DaemonState::Foreign { pid }`** (PID aus `.lock`-Datei):
    - Header-Button disabled, Label „läuft extern (PID x)"
    - Tray-Toggle ebenso
    - `on_exit`/`shutdown_daemon`/PDEATHSIG fassen ihn nicht an (kein Kind)
  - `Foreign` wird automatisch verlassen: im Poll-Tick `kill(pid, 0)` —
    `ESRCH` → zurück zu `Stopped` + Status „Externer Daemon beendet".
    PID-Reuse ist möglich, aber harmlos: der echte Konflikt-Schutz bleibt der
    Daemon-Flock beim nächsten `mount()`.
- Der Doppel-Daemon-Schutz existiert DAEMON-seitig (W4, gleiche ts pro
  dump_dir unmöglich). systemd-Unit (/var/lib/crashmon) und GUI-Daemon
  (~/.local/share/crashmon) nutzen verschiedene dump_dirs → keine
  Report-Doppelung. Der GUI-Check ist UX, nicht Schutz.

### Waisen-Schutz (PDEATHSIG)

- `spawn_daemon` bekommt einen `pre_exec`-Hook (`std::os::unix::process::
  CommandExt`):
  1. gemerktes `getppid()` vor dem Spawn
  2. `prctl(PR_SET_PDEATHSIG, SIGTERM)`
  3. `getppid()` erneut — bei Abweichung `_exit(0)` (Race geschlossen:
     Parent starb zwischen fork und prctl; alle drei Calls sind
     async-signal-safe)
- **WARUM-Kommentar an `spawn_daemon`:** PDEATHSIG hängt am THREAD, nicht am
  Prozess — stirbt der forked-habende Thread, feuert es sofort. Heute spawnt
  der Main-Thread, der lebt so lange wie die App. Landmine dokumentieren
  für den Tag, an dem der Spawn in einen Worker-Thread wandert.
- Effekt: kill -9, Logout, OOM, Panic → Kernel beendet den Daemon mit
  SIGTERM (Daemon hat Drain+Flush auf SIGTERM, E2E-verifiziert).
- `on_exit` → `shutdown_daemon` bleibt der saubere Pfad (geordneter Exit);
  PDEATHSIG ist das Netz darunter.

## Quit vs. Verstecken

- `quitting: bool` (default false) + `tray_active: bool` im GUI-State.
- Reine Funktion, Unit-getestet:
  `fn close_action(tray_active: bool, quitting: bool) -> CloseAction`
  - `(false, _)` → `Proceed` (App endet normal, `on_exit` → `shutdown_daemon`)
  - `(true, false)` → `Hide` (`CancelClose` + `Visible(false)`)
  - `(true, true)` → `Proceed` (Tray-Quit-Pfad — ohne diese Kombination
    käme man über das Tray nie raus)
- Tray „Beenden" → Sender `Quit` → GUI: `quitting = true`, dann
  `send_viewport_cmd(Close)` → App endet → `on_exit` → `shutdown_daemon`.

## Tray

- **Icon:** eingebettete `icon_pixmap` (16+22 px ARGB, Code-generiert:
  neutral = graues Crash-Icon; Alarm-Variante wenn neue Reports seit dem
  letzten sichtbaren Zeitpunkt ankamen — via `handle.update()` am
  Report-Flanke). `icon_name` LEER lassen — bei
  SNI hat `IconName` Vorrang vor `IconPixmap`; ein unbekannter Name kann je
  nach Host in „gar kein Icon" enden statt auf das Pixmap zu fallen.
  ARGB32 ist in der SNI-Spec big-endian — falls Farben vertauscht
  ankommen, ist das die Ursache, nicht der Generator.
- Menü:
  - „Fenster anzeigen" → Sender `Show` → GUI: `Visible(true)` +
    `ViewportCommand::Focus` + `Minimized(false)` (Visible allein mappt nur;
    je nach WM liegt das Fenster sonst hinter allem). Wayland kann
    Fokus-Request verweigern (xdg-activation) — Verhalten auf dem echten
    Stack testen, dokumentieren.
  - Daemon-Toggle („Daemon stoppen"/„starten", Label vom
    DaemonState-Spiegel) → Sender `ToggleDaemon` → GUI: `mount()`/`stop()`.
  - „Beenden" → Sender `Quit` (s.o.)
- **`handle.update()` NUR an Zustandsflanken** (DaemonState-Übergang in
  `poll_daemon`/`mount()`/`stop()`, Report-Flanke für Alarm-Icon) —
  **NIEMALS im Poll-Tick**: die blocking-API macht einen D-Bus-Roundtrip,
  im 500-ms-Tick hängt das den GUI-Thread 2× pro Sekunde.

## Fenstergröße

- `if !hidden { persist }` — die `InnerSize`-Persistenz läuft **nur wenn das
  Fenster sichtbar ist**. `logic()` tickt auch hidden weiter; ein verstecktes
  Viewport meldet keine garantierte letzte sichtbare Größe → sonst
  „Briefmarken-Fenster" beim nächsten Start.

## Testing

- `close_action`: Unit-Tests aller 4 Kombinationen (inkl.
  `(true, true)` → `Proceed`).
- Tray-Struct: pure, testbar ohne D-Bus — `menu()`-Items + Aktivierungs-
  Callbacks senden `TrayCmd` über mpsc, Test mit echtem Receiver.
- Icon-Generator: pure Funktion, gibt ARGB-Bytes zurück — Pixel-Smoke-Test
  (Kreis-Fläche nicht leer, Alarm != neutral).
- **PDEATHSIG-E2E als eigenes `[[bin]] pdeath_helper`** (kein env-Flag im
  Produktions-`main()` — Testcode aus dem Produktionspfad halten, k8-
  Lehre): Helper spawnt Kind (mit `spawn_daemon`-pre_exec), schreibt PID
  auf stdout, `SIGKILL` auf sich selbst. Test (via
  `env!("CARGO_BIN_EXE_pdeath_helper")`) pollt, ob das Kind stirbt —
  **mit Timeout** (sonst hängt CI, wenn PDEATHSIG nicht feuert).
- Bestandstests unverändert grün (state.rs inkl. `DaemonState::Foreign`
  in der Zustandsmaschine, kittest, scan).
- Manueller Smoke auf dem echten Stack (KDE Wayland):
  X → Hide, Tray zeigen (Fokus-Verhalten)/Beenden, kill -9 auf GUI →
  `pgrep crashmon` leer.

## Bewusst raus (YAGNI)

- Daemon-Exit-Notification im Hidden-Zustand (Daemon-Supervision ist nicht
  der Tray-Zweck).
- Autostart der GUI selbst beim Login.
- Poll-Intervall-Streckung im Hidden-Zustand (Messung: unnötig).
