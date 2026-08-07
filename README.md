# crashmon: Linux Crash-Daemon & GUI

**Dein Crash versteht dich nicht, aber du kannst ihn verstehen lernen.**

Wenn auf Linux etwas abstürzt, bleibt meist nur ein kryptischer Log-Eintrag:
`NVRM: Xid 31`, `amdgpu: GPU reset begin!`, `Out of memory: Killed process 1234`.
Was heißt das? Wer war das? Und was macht man jetzt?

crashmon beantwortet das automatisch. Es überwacht das System passiv
(~0 % Idle-CPU), erkennt Crashes, GPU-Hänger und OOM-Kills, bündelt sie zu
einem verständlichen Report und erklärt dir, was passiert ist. Und wenn
etwas **Neues** auftaucht, das noch niemand erklärt hat: Es schlägt selbst
nach und lernt daraus: deine Wissensdatenbank wächst mit jedem Fehler.

## Die Idee dahinter

Crashes sind selten dokumentiert, und wenn, dann verstreut über Foren,
Doku-Seiten und Treiber-Quelltexte. Dieses Tool entstand aus dem Frust,
beim eigenen Crash vor einem Xid-Code zu sitzen, ohne zu wissen, was er
bedeutet. Statt jedes Mal zu googeln, soll das System selbst sprechen:

- **Erfassen**: was ist passiert? (Coredump, OOM, GPU-Reset, Xid, Wedged)
- **Verstehen**: was bedeutet das? (eingebaute Referenz + Wissensspeicher)
- **Lernen**: unbekannte Fehler werden nachgeschlagen und in die eigene
  Wissensdatenbank übernommen; Community-Beiträge sind später per
  Pull-Request gegen die Repo-Wissensdatei möglich

## Was es kann

- **Daemon** (headless, unprivilegiert): liest systemd-Journal (Coredumps,
  OOM, GPU-Resets aller Hersteller) + Netlink-Uevents (GPU-WEDGED),
  korreliert (Xid-Bursts = 1 Report, PID-Matching) und schreibt atomare
  JSON-Reports
- **GUI** (egui/eframe): „Daemon mounten"-Knopf (kein Root, kein systemctl),
  Live-Report-Liste, formatierte Detail-Ansicht, Daemon-Log, JSON-Kopieren
- **Wissensbasis**: eingebaute Referenz (Xid-Codes, Signale, typische
  Meldungen) + lokaler, editierbarer Wissensspeicher (`knowledge.md`), der
  sich automatisch um neue Vorlagen-Sektionen erweitert und bei unbekannten
  Fehlern selbst nachschlägt (automatisches Nachschlagen über die
  DuckDuckGo-API)

## Schnellstart

```sh
# Voraussetzungen: Rust, systemd-libs (Arch: sudo pacman -S systemd-libs)
cargo build --release

# 1) GUI starten:
cargo run --release -p crashmon-gui

# 2) Im Fenster: „▶ Daemon starten"
# 3) Einen Crash erzeugen (zweites Terminal):
ulimit -c unlimited
sh -c 'kill -SEGV $$'
# 4) Nach wenigen Sekunden erscheint der Report live in der Liste
```

Ohne GUI (Daemon direkt):

```sh
cargo run --release -- --config config.example.toml --dump-dir /tmp/crashmon
```

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
  Journal-Rotationen
- **GUI** zeigt die Reports formatiert (nicht rohes JSON) mit Referenz
- **Automatisches Nachschlagen**: unbekannter Xid-Code → DuckDuckGo-Abfrage
  (einmalig pro Code, Hintergrund-Thread) → Ergebnis + Quelle wird an
  `knowledge.md` angehängt (markiert als „Auto-gelernt")

## GUI im Detail

| Bereich | Funktion |
|---|---|
| Kopf | Daemon starten/stoppen (SIGTERM = sauberer Shutdown mit Drain+Flush), Status |
| Links | Report-Liste (neueste oben, 🆕-Badge, Auto-Select) |
| Mitte | Detail: Event-Felder, 📋 JSON kopieren, Referenz (Severity + Erklärung), Wissensspeicher (klappbar) |
| Unten | Daemon-Log (live, Tail) |

Fenster schließen bei laufendem Daemon → Daemon wird sauber mitbeendet
(kein Waisen-Prozess). Vulkan-Warnung (`radv is not a conformant...`) beim
Start ist auf AMD normal und harmlos.

## Wissensspeicher & automatisches Nachschlagen

- **Repo-Vorlage** `crashmon-gui/knowledge.md`: versionierbar, editierbar;
  beim Bauen eingebettet
- **Laufzeit-Instanz** `~/.local/share/crashmon/knowledge.md`: deine
  Einträge + nachgeschlagene Ergebnisse; wird **nie überschrieben**, aber
  automatisch um fehlende Vorlagen-Sektionen **erweitert** (Merge bei
  jedem Start)
- Neue Xid-Codes, Signale, Meldungen einfach selbst eintragen, die GUI
  zeigt sie sofort
- Community: Inhalte per Pull-Request gegen die Repo-Datei teilen

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

**4. WEDGED-Uevent (root nötig):**
```sh
sudo udevadm trigger --subsystem-match=drm --action=change --property-match=WEDGED=bus-reset
```

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
cargo build          # Daemon (Workspace-Default)
cargo build -p crashmon-gui
cargo test           # Daemon-Tests (69)
cargo test -p crashmon-gui   # GUI-Tests (37, headless inkl. Klick-Smoke)
cargo clippy --all-targets -- -D warnings
```

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
                      reference (Wissensbasis), fetch (Auto-Learning)
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
- Das automatische Nachschlagen nutzt die DuckDuckGo-Instant-Answer-API;
  für sehr spezielle Fehler liefert sie oft nichts; dann hilft ein
  eigener Eintrag in `knowledge.md`
- Unbekannte Xid-Codes ohne gesicherte Quelle werden bewusst nicht
  erraten; sie bleiben dem Nachschlagen oder dir überlassen
