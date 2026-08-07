# Contributing

Kurzfassung: Issues/Pull-Requests sind willkommen. Bitte zwei Dinge beachten:

## 1. Commit- und Review-Erwartungen

- Conventional Commits (`feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `chore:`).
- `cargo fmt` und `cargo clippy --all-targets -D warnings` muessen lokal gruen sein
  (CI prueft beides).
- Neue Muster/Fixtures brauchen eine belegbare Herkunft — siehe unten.
- Alle Abhaengigkeiten exakt pinnen (`=`), Toolchain laut `rust-toolchain.toml`
  (1.95.0).

## 2. knowledge.md-Beitraege (maschinenlesbar bleiben)

`crashmon-gui/knowledge.md` ist die kuratierte Wissensbasis, die die GUI
einbettet und mit der lokalen Datei mergt. Damit Beitraege maschinenlesbar
bleiben:

- Neue Xid-Codes in der Sektion `## Weitere Xid-Codes` als
  `- <code>: <severity> — <Beschreibung>` ergaenzen
  (Severity: `fatal | kritisch | hoch | mittel`).
- Keine Zeilen, die nicht mindestens ein `-`-Listenelement sind.
- Die Code-Tabelle in `src/gpu/matcher.rs` (Funktion `xid_info`) ist die
  einzige Quelle fuer die GUI — ein knowledge.md-Eintrag allein reicht nicht,
  damit die Severity-Farbe erscheint; beide Stellen pflegen.

## Fixtures aus echten Quellen

Kernelzeilen in `tests/fixtures/kernel_lines.txt` sind copy-paste aus echten
Bugreports (Quelle im Kommentar). Selbst getippte Beispielzeilen werden
abgelehnt — sie haben in der Vergangenheit genau die Bugs erzeugt, die der
Matcher heute testet.

## Lizenz

MIT — siehe `LICENSE`. Mit einem PR akzeptierst du, dass dein Beitrag unter
dieser Lizenz steht.
