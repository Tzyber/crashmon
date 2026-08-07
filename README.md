# crashmon: Linux Crash-Daemon & GUI

**Dein Crash versteht dich nicht, aber du kannst ihn verstehen lernen.**

Wenn auf Linux etwas abstürzt, bleibt meist nur ein kryptischer Log-Eintrag:
`NVRM: Xid 31`, `amdgpu: GPU reset begin!`, `Out of memory: Killed process 1234`.
Was heißt das? Wer war das? Und was macht man jetzt?

crashmon beantwortet das automatisch. Es überwacht das System passiv
(~0 % Idle-CPU), erkennt Crashes, GPU-Hänger und OOM-Kills, bündelt sie zu
einem verständlichen Report und erklärt dir, was passiert ist. Neue
Crashes meldet es per Desktop-Notification — und bei unbekannten Fehlern
öffnet ein Knopf die vorformulierte Suche im Browser, damit deine
Wissensdatenbank mit jedem Fehler wächst.

## Die Idee dahinter

Crashes sind selten dokumentiert, und wenn, dann verstreut über Foren,
Doku-Seiten und Treiber-Quelltexte. Dieses Tool entstand aus dem Frust,
beim eigenen Crash vor einem Xid-Code zu sitzen, ohne zu wissen, was er
bedeutet. Statt jedes Mal zu googeln, soll das System selbst sprechen:

- **Erfassen**: was ist passiert? (Coredump, OOM, GPU-Reset, Xid, Wedged)
- **Verstehen**: was bedeutet das? (eingebaute Referenz + Wissensspeicher)
- **Lernen**: unbekannte Fehler per Knopf im Browser nachschlagen und
  selbst in die Wissensdatei übernehmen; Community-Beiträge per
  Pull-Request gegen die Repo-Wissensdatei (Format: CONTRIBUTING.md)

## Was es kann

- **Daemon** (headless, unprivilegiert): liest systemd-Journal (Coredumps,
  OOM, GPU-Resets aller Hersteller) + Netlink-Uevents (GPU-WEDGED),
  korreliert (Xid-Bursts = 1 Report, PID-Matching) und schreibt atomare
  JSON-Reports
- **GUI** (egui/eframe): „Daemon mounten"-Knopf (kein Root, kein systemctl),
  Live-Report-Liste, formatierte Detail-Ansicht, Daemon-Log, JSON-Kopieren
- **Wissensbasis**: eingebaute Referenz (Xid-Codes, Signale, typische
  Meldungen) + lokaler, editierbarer Wissensspeicher (`knowledge.md`), der
  sich automatisch um neue Vorlagen-Sektionen erweitert; unbekannte Fehler
  werden per „Im Browser suchen"-Knopf nachgeschlagen (kein automatischer
  Netzwerkverkehr)
- **Desktop-Notification** bei neuem Report (`notify-send`)

## Schnellstart

```sh
# Voraussetzungen: Rust, systemd-libs (Arch: sudo pacman -S systemd-libs)
cargo build --release

# 1) GUI starten:
cargo run --release -p crashmon-gui

# 2) Im Fenster: „Daemon starten"
# 3) Einen Crash erzeugen (zweites Terminal):
ulimit -c unlimited
sh -c 'kill -SEGV $$'
# 4) Nach wenigen Sekunden erscheint der Report live in der Liste (+ Notification)
```

Ohne GUI (Daemon direkt):

```sh
cargo run --release --bin crashmon -- --config config.example.toml --dump-dir /tmp/crashmon
```

Zwei Daemon-Instanzen auf demselben `dump_dir` sind durch `flock`
(`.lock`) ausgeschlossen — die zweite bricht mit klarer Meldung ab.

Alle Daten liegen unter `~/.local/share/crashmon/`:
`crash-<ts>.json` (Reports), `config.toml`, `crashmon-daemon.log`,
`knowledge.md` (Wissensspeicher).

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

- **Journal** wird über den sd-journal-FD gelesen (epoll-Park, kein Polling);
  Cursor-Persistenz überlebt Neustarts, periodisches Re-Open heilt
  Journal-Rotationen; ein amdgpu-Hang mit 20–50 Kernelzeilen wird in einem
  Rutsch abgearbeitet (Budget + `yield_now`, kein 2-Einträge-pro-Minute-Tröpfeln)
- **GUI** zeigt die Reports formatiert (nicht rohes JSON) mit Referenz;
  Scan/Lesefestplatte sind entprellt (500 ms) und parsen bekannte Reports
  gar nicht erst neu
- **Im Browser suchen**: unbekannter Xid-Code → `xdg-open` mit
  vorformulierter Suche; du entscheidest, was in `knowledge.md` wandert

## GUI im Detail

| Bereich | Funktion |
|---|---|
| Kopf | Daemon starten/stoppen (SIGTERM = sauberer Shutdown mit Drain+Flush), Log + Wissensspeicher umschalten |
| Links | Report-Liste (neueste oben, Severity-Punkt, „vor X Min", Filter), Rechtsklick löscht einen Report |
| Mitte | Detail: Event-Felder (Grid), JSON kopieren, Referenz (Severity + Erklärung, farbig), „Im Browser suchen" bei unbekannten Xids |
| Unten | Status-Leiste (immer sichtbar); Daemon-Log aufklappbar |
| Extra | Wissensspeicher in eigenem Fenster („Neu laden" bei externer Bearbeitung) |

Fenster schließen bei laufendem Daemon → Daemon wird sauber mitbeendet
(kein Waisen-Prozess); die Fenstergröße wird gemerkt. Neue Reports lösen
eine Desktop-Notification aus (via `notify-send`, sonst still deaktiviert).
Vulkan-Warnung (`radv is not a conformant...`) beim Start ist auf AMD
normal und harmlos.

## Wissensspeicher & Nachschlagen

- **Repo-Vorlage** `crashmon-gui/knowledge.md`: versionierbar, editierbar;
  beim Bauen eingebettet
- **Laufzeit-Instanz** `~/.local/share/crashmon/knowledge.md`: deine
  Einträge; wird **nie überschrieben**, aber automatisch um fehlende
  Vorlagen-Sektionen **erweitert** (Merge beim Start / „Neu laden")
- Neue Xid-Codes, Signale, Meldungen einfach selbst eintragen, die GUI
  zeigt sie sofort
- Bei unbekannten Xids: „Im Browser suchen" (vorformulierte Query, kein
  automatischer Netzwerkverkehr aus der App)
- Community: Inhalte per Pull-Request gegen die Repo-Datei teilen
  (Format-Regeln: CONTRIBUTING.md)

## Testrezepte (Verifikation)

Alle Crashes sind deterministisch; die ersten drei laufen unprivilegiert.

**1. Coredump (E2E):**
```sh
cargo run -- --config config.example.toml --dump-dir /tmp/crashmon &
ulimit -c unlimited && sh -c 'kill -SEGV $$'
sleep 30; ls /tmp/crashmon/crash-*.json
```
Hinweis: systemd-coredump kann den Journal-Eintrag lastabhängig um
10–20 s verzögern; Wartezeiten großzügig wählen.

**2. OOM-Killer:** Kern-Zeilen sind nicht ohne Root injizierbar; der
Matcher ist via `tests/matcher_test.rs` deterministisch abgedeckt; reale
Zeilen erscheinen automatisch als `OomKill`-Reports.

**3. NVIDIA-Xid:** ebenso kernel-only; erkennbare reale Zeilen:
```sh
journalctl _TRANSPORT=kernel | grep -i "NVRM: Xid"
```

**4. WEDGED-Uevent:** nicht deterministisch testbar — `udevadm trigger
--property-match` filtert nur (kein Gerät hat ein `WEDGED`-Property,
also passiert nichts), und der Kernel akzeptiert keine Custom-Properties
per sysfs-Uevent-Write (EINVAL). Echte Wedge-Events kommen nur vom
xe-Treiber; der Parser ist über `tests/uevent_test.rs` + den
`xe ... as wedged`-Journal-Fixture abgedeckt.

**5. Service-Unit (Produktiv):**
```sh
sudo useradd --system --home /var/lib/crashmon --shell /usr/sbin/nologin crashmon
sudo cp systemd/crashmon.service /etc/systemd/system/
sudo install -Dm644 config.example.toml /etc/crashmon/config.toml
sudo systemctl daemon-reload && sudo systemctl enable --now crashmon
```

## Report-Format

`crash-<ts>.json` (ts = UTC-µs der Ursache), atomar geschrieben:

```json
{
  "ts": 1786127513476706,
  "cause": { "ts": ..., "event": { "kind": "Coredump", "data": { "pid": 42, "signal": "SIGSEGV", ... } } },
  "related": [],
  "lost_events": 0
}
```

`cause` = frühester Event der Gruppe (T0-Prinzip), `related` = beigeordnete
(Folge-Xids, korrelierte Coredumps), `lost_events` = kumulative Verluste
durch Kanal-Überlauf seit Start.

## Build & Test

```sh
cargo build          # Daemon (Binary: crashmon; Workspace-Default)
cargo build -p crashmon-gui
cargo test           # Daemon-Tests
cargo test -p crashmon-gui   # GUI-Tests (headless inkl. Klick-Smoke)
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

Toolchain ist in `rust-toolchain.toml` gepinnt (1.95.0, MSRV des
Workspace — egui/eframe 0.36 verlangt >= 1.95).

Host-Tests (brauchen echtes Journal + systemd-coredump):
```sh
cargo test --test e2e_test -- --ignored --nocapture
cargo test --test journal_smoke -- --ignored --nocapture
```

## Struktur

```
src/                  Daemon (lib + Binary)
  daemon.rs           Loops + Shutdown-Sequenz + Journal-Re-Open
  ingest/             Journal-Ingestion (Cursor-Persistenz)
  gpu/                Matcher (Xid/OOM/Reset) + Uevent-Listener
  aggregate.rs        Korrelation + Backpressure
  output.rs           JSON-Report-Writer (atomar)
crashmon-gui/         GUI (egui/eframe, eigenes Workspace-Crate)
  src/                app, state (Prozess-Lifecycle), scan, format,
                      logtail, reference (Wissensbasis)
  knowledge.md        Wissensspeicher-Vorlage (versionierbar)
systemd/              Gehärtete Service-Unit
openspec/specs/       Capability-Spezifikationen (SDD)
```

Benötigt libsystemd-Dev-Dateien (`libsystemd-sys` via pkg-config; auf Arch:
Paket `systemd-libs`).

## Aktive Entwicklung

Dieses Projekt wird aktiv weiterentwickelt: Software, die stehen bleibt,
veraltet. Geplante/regelmäßige Arbeit:

- Neue Fehlermuster und Xid-Codes in der Wissensbasis (auch aus eigenen
  Crashes, die das Tool dokumentiert)
- Treiber-/Kernel-Entwicklungen verfolgen (neue Wedged-Methoden, neue
  Fehlermeldungen)
- Community-Beiträge: Issues, Pull-Requests und Wissensdatei-Erweiterungen
  sind willkommen

## Bekannte Grenzen

- systemd-coredump kann den Journal-Eintrag eines Crashes lastabhängig um
  10–20 s verzögern; Reports erscheinen entsprechend später
- Desktop-Notifications brauchen `notify-send` (libnotify); ohne das Tool
  ist nur die GUI die Benachrichtigung
- Unbekannte Xid-Codes ohne gesicherte Quelle werden bewusst nicht
  erraten; sie bleiben dem Browser-Nachschlagen oder dir überlassen
- Die GUI hat noch keinen Tray-/Hintergrund-Modus: Fenster schließen
  beendet den Daemon mit (geplant: Fenster verstecken + Tray-Icon)
