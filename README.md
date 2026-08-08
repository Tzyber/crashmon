# crashmon

Crash-Daemon für Linux. Überwacht systemd-coredump und das Journal und macht
daraus lesbare Reports. Mit Desktop-GUI, ohne Root.

> Die Oberfläche ist auf Deutsch.

Wenn auf Linux etwas abstürzt, bleibt meist eine einzelne Zeile irgendwo im
Journal:

```
NVRM: Xid 31
amdgpu: GPU reset begin!
Out of memory: Killed process 1234
```

Man muss wissen, dass es sie gibt, man muss sie finden, und dann sagt sie
einem immer noch nichts. crashmon nimmt einem beides ab: es liest passiv mit
(praktisch 0 % CPU im Leerlauf), erkennt Coredumps, OOM-Kills und GPU-Hänger,
fasst zusammengehörende Ereignisse zu einem Report zusammen und erklärt dazu,
was der Fehler bedeutet. Bei neuen Reports kommt eine Desktop-Notification.
Bei unbekannten Fehlern öffnet ein Knopf die vorformulierte Suche im Browser,
und was du herausfindest, kannst du in deinen Wissensspeicher schreiben.

![Die crashmon-GUI mit zwei Reports: links die Liste, rechts die Detailansicht eines SIGABRT mit Programmpfad, Signal, Core-Datei und Erklaerung](Docs/images/gui-report.png)

## Was es kann

- **Daemon** (headless, unprivilegiert): liest das systemd-Journal (Coredumps,
  OOM, GPU-Resets aller Hersteller) und Netlink-Uevents (GPU-WEDGED),
  korreliert zusammengehörende Ereignisse (Xid-Bursts werden ein Report,
  PID-Matching) und schreibt atomare JSON-Reports.
- **GUI** (egui/eframe): startet und stoppt den Daemon ohne Root und ohne
  systemctl, zeigt die Reports live, formatiert statt roh, dazu Daemon-Log
  und einen Knopf zum JSON-Kopieren.
- **Tray-Modus**: Fenster zu, GUI läuft weiter und meldet neue Crashes.
- **Wissensbasis**: eingebaute Referenz (Xid-Codes, Signale, typische
  Meldungen) plus ein lokaler, editierbarer Wissensspeicher (`knowledge.md`),
  der beim Start um fehlende Vorlagen-Sektionen ergänzt wird. Aus der App
  geht kein Netzwerkverkehr raus, nachgeschlagen wird nur, wenn du auf
  "Im Browser suchen" drückst.

## Schnellstart

Voraussetzungen: Rust (Toolchain ist gepinnt, siehe `rust-toolchain.toml`),
die systemd-Dev-Dateien (Arch: `sudo pacman -S systemd-libs`) und für die
Benachrichtigungen `notify-send` aus libnotify. Ohne libnotify läuft alles,
nur eben still.

```sh
cargo build --release
cargo run --release -p crashmon-gui
```

Der Daemon startet automatisch mit der GUI. Steht in der Statusleiste eine
PID, läuft er.

Zum Ausprobieren einen echten Crash erzeugen:

```sh
cat /proc/sys/kernel/core_pattern    # muss auf systemd-coredump zeigen
ulimit -c unlimited
sh -c 'kill -SEGV $$'
```

Der Report erscheint in der Liste, sobald systemd-coredump den Eintrag ins
Journal geschrieben hat. Das dauert je nach Systemlast bis zu 20 Sekunden,
also nicht zu früh aufgeben. Zeigt `core_pattern` auf etwas anderes als
systemd-coredump, entsteht zwar ein Crash, aber kein Journal-Eintrag, und
damit auch kein Report.

Ohne GUI, nur der Daemon:

```sh
cargo run --release --bin crashmon -- --config config.example.toml --dump-dir /tmp/crashmon
```

Zwei Daemon-Instanzen auf demselben `dump_dir` schließt ein `flock` auf
`.lock` aus, die zweite bricht mit einer klaren Meldung ab.

Alle Daten liegen unter `~/.local/share/crashmon/`: die Reports als
`crash-<ts>.json`, dazu `config.toml`, `crashmon-daemon.log` und
`knowledge.md`.

## So funktioniert es

```
Journal (sd-journal) ─┐
  Coredumps (MESSAGE_ID) ──► Matcher ─► Events ─┐
  Kernel (OOM, GPU-Reset, Xid)                   │
Uevents (Netlink, WEDGED) ──────────────────────┤
                                                 ▼
                                    Aggregator (Korrelation)
                                      ├─ Xid-Bursts → 1 Report (T0-Prinzip)
                                      ├─ PID + 5-s-Fenster → 1 Report
                                      └─ bounded Kanal (drop-newest + Lost-Zähler)
                                                 ▼
                                    crash-<ts>.json (atomar, µs-UTC)
```

Das Journal wird über den sd-journal-FD gelesen, per epoll geparkt statt
gepollt. Der Cursor wird persistiert und überlebt Neustarts, ein
periodisches Re-Open heilt Journal-Rotationen. Ein amdgpu-Hang schreibt 20
bis 50 Kernelzeilen auf einmal, die in einem Rutsch abgearbeitet werden
(Budget plus `yield_now`), statt in zwei Einträgen pro Minute zu tröpfeln.

Die GUI hält sich zurück: Scannen und Lesen sind auf 500 ms entprellt, und
bereits bekannte Reports werden gar nicht erst neu geparst.

## GUI im Detail

| Bereich | Funktion |
|---|---|
| Kopf | Daemon starten und stoppen (SIGTERM, also sauberer Shutdown mit Drain und Flush), Log und Wissensspeicher umschalten |
| Links | Report-Liste, neueste oben, mit Severity-Punkt, "vor X Min" und Filter. Rechtsklick löscht einen Report |
| Mitte | Detail: alle Event-Felder, JSON kopieren, Referenz mit Severity und Erklärung, bei unbekannten Xids "Im Browser suchen" |
| Unten | Statusleiste, immer sichtbar. Daemon-Log aufklappbar |
| Extra | Wissensspeicher in einem eigenen Fenster, mit "Neu laden" für externe Änderungen |

Die Fenstergröße wird gemerkt. Die Vulkan-Warnung beim Start
(`radv is not a conformant...`) ist auf AMD normal und harmlos.

## Tray-Modus

Beim Schließen des Fensters verschwindet die GUI ins Systemtray
(StatusNotifierItem) und überwacht den Report-Ordner weiter. Beendet wird
über das Tray-Menü.

- KDE Plasma und COSMIC: funktioniert direkt.
- GNOME: braucht die AppIndicator-Extension.
- Wayland: das Fenster wird minimiert, nicht wirklich versteckt. Kein
  Toolkit kann das dort, `set_visible` ist in winit unter Wayland nicht
  unterstützt. Zurück kommst du über das Taskbar-Icon. Der Menüpunkt
  "Fenster anzeigen" wirkt nur auf X11, weil Wayland kein Unminimize kennt.
- Ohne StatusNotifierHost startet die GUI ohne Tray, dann beendet das
  Fenster-X die App wie gewohnt.
- Läuft bereits ein Daemon auf demselben `dump_dir`, zum Beispiel über die
  systemd-Unit, erkennt die GUI das und startet keinen zweiten.
- Stirbt die GUI ungeordnet, etwa durch `kill -9` oder Logout, beendet der
  Kernel den Daemon per PDEATHSIG mit SIGTERM. Es bleibt kein Waisenprozess
  zurück.

## Wissensspeicher

Es gibt zwei Dateien. `crashmon-gui/knowledge.md` liegt im Repo, ist
versionierbar und wird beim Bauen eingebettet.
`~/.local/share/crashmon/knowledge.md` ist deine Kopie, sie wird nie
überschrieben, sondern beim Start nur um Sektionen ergänzt, die in der
Vorlage neu dazugekommen sind.

Neue Xid-Codes, Signale oder Meldungen trägst du einfach selbst ein, die GUI
zeigt sie sofort. Wenn du etwas herausgefunden hast, das anderen hilft:
Pull-Request gegen die Repo-Datei, die Format-Regeln stehen in
CONTRIBUTING.md.

## Verifikation

Der Coredump-Pfad lässt sich unprivilegiert komplett durchtesten:

```sh
cargo run -- --config config.example.toml --dump-dir /tmp/crashmon &
ulimit -c unlimited && sh -c 'kill -SEGV $$'
sleep 30; ls /tmp/crashmon/crash-*.json
```

OOM-Kills, Xids und Wedge-Events kann man nicht ohne Weiteres von Hand
auslösen, die Zeilen kommen nur vom Kernel selbst. Die Matcher dafür sind
über Fixtures abgedeckt (`tests/matcher_test.rs`, `tests/uevent_test.rs`);
Details dazu in CONTRIBUTING.md. Ob dein System reale Xids meldet, siehst du
mit:

```sh
journalctl _TRANSPORT=kernel | grep -i "NVRM: Xid"
```

Als Systemdienst, ohne GUI:

```sh
sudo useradd --system --home /var/lib/crashmon --shell /usr/sbin/nologin crashmon
sudo cp systemd/crashmon.service /etc/systemd/system/
sudo install -Dm644 config.example.toml /etc/crashmon/config.toml
sudo systemctl daemon-reload && sudo systemctl enable --now crashmon
```

## Report-Format

`crash-<ts>.json`, wobei `ts` die UTC-Mikrosekunden der Ursache sind. Die
Datei wird atomar geschrieben.

```json
{
  "ts": 1786127513476706,
  "cause": { "ts": ..., "event": { "kind": "Coredump", "data": { "pid": 42, "signal": "SIGSEGV", ... } } },
  "related": [],
  "lost_events": 0
}
```

`cause` ist das früheste Ereignis der Gruppe (T0-Prinzip), `related` sind die
beigeordneten (Folge-Xids, korrelierte Coredumps), und `lost_events` zählt
kumulativ, wie viele Ereignisse seit dem Start durch Kanal-Überlauf verloren
gingen.

## Build und Test

```sh
cargo build                  # Daemon (Binary: crashmon, Workspace-Default)
cargo build -p crashmon-gui
cargo test                   # Daemon-Tests
cargo test -p crashmon-gui   # GUI-Tests, headless inklusive Klick-Smoke
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

Tests, die ein echtes Journal und systemd-coredump brauchen, laufen nur auf
Anforderung:

```sh
cargo test --test e2e_test -- --ignored --nocapture
cargo test --test journal_smoke -- --ignored --nocapture
```

## Struktur

```
src/                  Daemon (lib + Binary)
  daemon.rs           Loops, Shutdown-Sequenz, Journal-Re-Open
  ingest/             Journal-Ingestion mit Cursor-Persistenz
  gpu/                Matcher (Xid, OOM, Reset) + Uevent-Listener
  aggregate.rs        Korrelation + Backpressure
  output.rs           JSON-Report-Writer (atomar)
crashmon-gui/         GUI (egui/eframe, eigenes Workspace-Crate)
  src/                app, state (Prozess-Lifecycle), tray, scan,
                      format, logtail, config, reference
  knowledge.md        Wissensspeicher-Vorlage (versionierbar)
systemd/              Gehärtete Service-Unit
openspec/specs/       Capability-Spezifikationen (SDD)
```

## Grenzen

- systemd-coredump kann den Journal-Eintrag je nach Last um bis zu 20
  Sekunden verzögern. Der Report kommt dann eben später.
- Unbekannte Xid-Codes werden nicht geraten. Wenn es keine gesicherte Quelle
  gibt, bleibt der Eintrag leer, und du bekommst stattdessen den Knopf zum
  Nachschlagen.
- Ohne libnotify gibt es keine Desktop-Benachrichtigung, dann ist die
  GUI-Liste die einzige Meldung.

## Warum ich das teile

Ich habe Abstürze auf meinem eigenen Rechner lange schlicht übersehen. Nicht
weil es keine Spuren gab, sondern weil die Spuren im Journal liegen, und das
Journal ist unübersichtlich. Man muss vorher wissen, wonach man sucht, um es
zu finden. So sieht ein einzelner Absturz dort aus:

![journalctl-Ausgabe im verbose-Format: ein einzelner Coredump als seitenlange Liste von Feldern](docs/images/journal-verbose.png)

Wenn ich die Zeile dann irgendwann hatte, stand da ein Code, eine Zahl, ein
Signalname, und ich war genauso schlau wie vorher. Also googeln,
Forenthreads von 2019, Treiber-Quelltext, halbe Antworten.

crashmon ist aus dieser Reihenfolge entstanden: erst wissen, dass überhaupt
etwas passiert ist, dann verstehen, was. Der Daemon sucht die Zeilen, die
Referenz erklärt sie, und was die Referenz nicht kennt, landet in einer
Wissensdatei, die ich selbst weiterschreiben kann.

Öffentlich ist es, weil genau dieses Wissen verstreut ist. Es steht in
Foren, in Kernel-Quellen, in den Köpfen von Leuten, die dasselbe Problem
schon hatten. Eine Wissensdatei im Repo, in die andere per Pull-Request
schreiben können, arbeitet dagegen. Jeder Eintrag, den jemand hinzufügt,
erspart dem Nächsten den Abend, den ich hatte.

Das Projekt ist in aktiver Arbeit. Es wird Fehler geben, und es fehlen mit
Sicherheit Fehlermuster, die ich noch nie gesehen habe. Issues und
Pull-Requests sind willkommen, besonders die mit echten Log-Zeilen darin.
