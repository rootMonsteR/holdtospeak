# bench/ — de-risk & regression harnesses

## injection-matrix (spike S3, minimal-but-real)

Verifies that dictated text actually lands in a target app, unaltered, without side effects — the
data behind the injection route table (`nib_platform::default_routes`). Grows toward the full
40-target grid in `docs/design/03-derisk-spike-program.md`.

For each target in `targets.toml` it injects a Unicode sentinel via the real route chain
(`nib-inject` + `nib-win32`'s `Win32Injector`) against the **actually focused** control
(`Win32TargetProbe`, not a synthetic profile), reads it back via UIA, and checks:

- **exact match** — the sentinel landed verbatim. Read-back tries `ValuePattern` then
  `TextPattern`; Win11's XAML Notepad exposes only the latter, so a ValuePattern-only read used to
  report a false FAIL for a perfectly good injection.
- **clipboard restored** — the user's clipboard is unchanged after the paste,
- **password refusal** — two levels: a *routing* cell (a synthetic `is_password` profile must yield
  `Refuse`) and a **live** cell (`live_refuse = true`) that focuses a real `<input type=password>`
  and requires UIA `IsPassword` to actually fire. The live one is what makes refusal evidence
  rather than an assumption — it fails loudly if detection misses.

Before injecting, a cell confirms the intended app really holds foreground; if focus-stealing
prevention left it in the background the cell FAILs rather than typing into the wrong window.

### Run it (needs a real desktop — it types into a launched app)

```
cargo run -p injection-matrix -- --targets bench/targets.toml
```

Automated cells (e.g. `notepad`) launch + inject + read back + self-clean. Cells marked
`manual = true` (Windows Terminal, Discord, a browser) are skipped until per-app launch/focus is
added — focus them yourself to extend coverage. Results print as a grid and write
`bench/injection-matrix-report.jsonl`.

**CI:** the crate is a workspace member, so `cargo build/test --workspace` compiles it and runs its
parsing/refusal unit tests on every push. The **live** SendInput cells are intentionally *not* in
`cargo test` (they need an interactive desktop), so CI stays deterministic; run them locally or on
the nightly physical bench.
