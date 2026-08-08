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
- **Tray-Verlust zur Laufzeit** (D-Bus-Neustart, Host-Wegfall): der
  Relevanzfall ist hidden + stabiler Daemon + keine Reports — tagelang
  keine Zustandsflanke. `update()`-Fehler reicht als Erkennung nicht
  (setzt Flanke voraus, die nie kommt). **Nativer ksni-Mechanismus statt
  Poll:** `Tray::watcher_offline(reason)` feuert im D-Bus-Thread, wenn
  der `org.kde.StatusNotifierWatcher` offline geht (verifiziert in
  ksni-0.3.6-Doku; `watcher_online` nur nach offline). Callback macht nur
  Sender-Push (`TrayCmd::TrayLost`/`TrayBack`), Rückgabe `true` (Service
  weiterlaufen lassen). GUI-Thread:
  - `TrayLost`: `tray_active = false`, **wenn `hidden`: `Visible(true)`**
    — einzige Reaktion, die den Prozess wieder erreichbar macht
    (Statuszeile im versteckten Fenster sieht niemand)
  - Statuszeile „Tray verloren — Fenster wieder gezeigt, X beendet die
    App"; `TrayBack`: `tray_active = true`
  - Ein `Handle::is_closed()`-Poll ist NICHT der richtige Mechanismus:
    „Handle geschlossen" ≠ „Host verschwunden" (Verbindung kann auf einen
    neuen Host warten)
- **Bekanntes Risiko:** `Handle::update()` ist blockierend (D-Bus-
  Roundtrip). Hängt der Bus (Host tot, Verbindung offen), hängt der
  GUI-Thread mit — im Hidden-Zustand unsichtbar. Kein Timeout drumherum
  (ksni bietet keinen); Update bleibt strikt auf Zustandsflanken + den
  alle-2-s-Liveness-Zweig.

## Daemon-Lifecycle

### Autostart + Single-Instance

- Daemon startet automatisch beim GUI-Start (User-Entscheidung Q1).
- **Vor `mount()`: Probe-Flock (`LOCK_EX|LOCK_NB`) auf `dump_dir/.lock`.**
  - Frei → Lock sofort wieder freigeben, Daemon normal spawnen (der Daemon
    selbst flockt danach — W4 `InstanceLock`, daemon.rs:48-89).
  - Belegt → **`DaemonState::Foreign { pid }`** (PID aus `.lock`-Datei,
    leer/0-Bytes möglich — der Daemon truncatet die Datei vor dem
    PID-Write; dann `pid: None`):
    - Header-Button disabled, Label „läuft extern (PID x)"
    - Tray-Toggle ebenso
    - `on_exit`/`shutdown_daemon`/PDEATHSIG fassen ihn nicht an (kein Kind)
  - **Semantik:** `is_running() == false` — die Frage heißt faktisch „haben
    wir ein Kind?"; der Button hängt am disabled-Pfad, nicht am Label.
    Jedes `match` über `DaemonState` bricht mit dem neuen Arm — erwartet,
    der Compiler listet alle Stellen.
  - `Foreign` wird automatisch verlassen: im Poll-Tick **Flock-Probe
    erneut** (`LOCK_EX|LOCK_NB` — autoritativ, statt PID-Lebendigkeits-Check:
    PID-Reuse und leere Lock-Datei machen den PID-Check unzuverlässig).
    Frei → zurück zu `Stopped` + Status „Externer Daemon beendet".
  - **Invariante:** die Flock-Probe läuft AUSSCHLIESSLICH im `Foreign`-
    Zweig — nie bei `Running`/`Stopping` (der eigene Daemon hält den Lock;
    eine Probe dort kippte den Zustand nach `Foreign`).
  - TOCTOU (zwei gleichzeitige GUI-Starts proben beide „frei"): der Verlierer
    spawnt, sein Daemon stirbt am W4-Flock sofort. Im Exit-Pfad von
    `poll_daemon` kurz nach Autostart erneut proben → wenn belegt:
    `Foreign` mit Sieger-PID statt generischer „Daemon beendet: exit status"-
    Meldung.
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
  `Visible(true)` (Close kann bei verstecktem/unmapped Fenster auf Wayland
  verpuffen) und erst danach `send_viewport_cmd(Close)` → App endet →
  `on_exit` → `shutdown_daemon`. Quit aus dem Hidden-Zustand ist der
  typische Tray-Moment — im Smoke-Test explizit prüfen.
- **Selbstheilend:** verpufft `Close` trotzdem, ist das Fenster jetzt
  sichtbar und `quitting == true` — der nächste X-Klick liefert `Proceed`.
  Absicht, nicht Zufall — Kommentar an die Stelle, damit es niemand
  „vereinfacht".

## Tray

- **Icon:** eingebettete `icon_pixmap` (16+22 px ARGB, Code-generiert:
  neutral = graues Crash-Icon; Alarm-Variante wenn neue Reports seit dem
  letzten sichtbaren Zeitpunkt ankamen). `icon_name` LEER lassen — bei
  SNI hat `IconName` Vorrang vor `IconPixmap`; ein unbekannter Name kann je
  nach Host in „gar kein Icon" enden statt auf das Pixmap zu fallen.
  ARGB32 ist in der SNI-Spec big-endian — falls Farben vertauscht
  ankommen, ist das die Ursache, nicht der Generator.
- **Alarm-Flanke ohne D-Bus im Poll-Tick:** neue Reports werden nur im
  500-ms-Tick erkannt (scan läuft dort) — `scan()` setzt daher nur ein
  `tray_dirty`-Flag; das eigentliche `handle.update()` fürs Alarm-Icon läuft
  in `logic()` außerhalb des Debounce-Zweigs (ein Roundtrip, nur wenn
  dirty). Marke „letzter sichtbarer Zeitpunkt" wird beim Hide und beim
  Show gesetzt.
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
  auf stdout, **`stdout().flush()` + Fehler prüfen** (Pipe ist
  block-buffered — SIGKILL flusht nichts; ohne Flush liest der Test eine
  leere Pipe und schlägt fehl, obwohl PDEATHSIG korrekt wäre), DANN
  `SIGKILL` auf sich selbst — Reihenfolge: spawn → PID schreiben →
  Flush → Kill. Test (via `env!("CARGO_BIN_EXE_pdeath_helper")`) pollt,
  ob das Kind stirbt — **mit Timeout** (sonst hängt CI, wenn PDEATHSIG
  nicht feuert).
  **Timeout-Pfad räumt auf:** wenn PDEATHSIG fehlschlägt (der zu testende
  Fall), läuft der Grandchild weiter und hält den Flock — der Test muss
  ihn dann per SIGKILL beenden + reapen, sonst verfälscht er den nächsten
  Lauf. Helper nutzt temp-dump_dir/temp-Config, nie den echten state_dir.
- Bestandstests grün nach den erwarteten `match`-Erweiterungen um
  `DaemonState::Foreign` (state.rs, kittest, scan — die neuen Arme sind
  vom Compiler vorgegeben).
- Manueller Smoke auf dem echten Stack (KDE Wayland):
  X → Hide, Tray zeigen (Fokus-Verhalten)/Beenden, kill -9 auf GUI →
  `pgrep crashmon` leer.

## Bewusst raus (YAGNI)

- Daemon-Exit-Notification im Hidden-Zustand (Daemon-Supervision ist nicht
  der Tray-Zweck).
- Autostart der GUI selbst beim Login.
- Poll-Intervall-Streckung im Hidden-Zustand (Messung: unnötig).
