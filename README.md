# HoldToSpeak — hold two keys, talk, it types

Local, private push-to-talk dictation for Windows. Hold **Ctrl + Win** anywhere, speak, let go —
your words appear at the cursor in whatever app you were already using.

**Your voice never leaves your machine.** No account, no cloud, no telemetry. After a one-time
model download it works with the network cable unplugged — and it's built so you can *verify* that
rather than take our word for it ([PRIVACY.md](PRIVACY.md)).

> Status: **early**. The dictation core works and is used daily by its author. Packaging, a
> settings UI, and installers are still landing. Bug reports welcome; rough edges expected.

## Why this exists

Cloud dictation is fast and good — and it ships your microphone to someone else's computer. That's
a non-starter in a lot of workplaces (legal, medical, finance, anywhere under an NDA), and it's
just unnecessary: a 2024-era speech model runs comfortably on a normal CPU.

## What it does

- **Works in the apps that usually break.** Terminals, WSL, IDEs, browsers, chat apps, remote
  sessions — via a per-target injection chain rather than clipboard-paste-and-hope.
- **Never rewrites your commands.** Focused on a terminal or code editor? Dictation drops to
  verbatim automatically, so `kubectl get pods -n kube-system` stays exactly that.
- **Refuses password fields.** Detected live via UI Automation — if the focused control is a
  credential field, nothing is typed.
- **Learns your jargon.** `learn cube ctl => kubectl` and it's fixed permanently, in plain text
  you can edit.
- **Tells you when it can't help.** Elevated window? You get "text kept, not inserted" — never a
  silently swallowed sentence.

## Install

Requires **Windows 10/11 (x64)** and a microphone.

```
git clone https://github.com/rootMonsteR/holdtospeak && cd holdtospeak
pwsh -File scripts/fetch-sherpa.ps1     # GPL-free speech runtime (~7 MB)
cargo build --release -p nib-core -p nib-asr-sidecar
cargo run --release -p nib-core -- --sidecar native
```

On first launch it downloads the speech model (~460 MB, once) and tells you it's doing so.
Installers are coming; for now this is a build-from-source affair.

## Use

| | |
|---|---|
| **Ctrl + Win** (hold) | dictate — speak, then release |
| **Ctrl + Alt + M** | cycle cleanup mode |
| tray icon | modes, overlay themes, quit |
| `learn <heard> => <meant>` | teach it a word, permanently |
| `q` + Enter | quit |

Rebind keys in `%APPDATA%\HoldToSpeak\hotkeys.toml`.

### Cleanup modes

- **Raw** — exactly what you said, plus your dictionary. Forced automatically in terminals/IDEs.
- **Auto** — deterministic tidy: fillers removed, sentence casing, terminal punctuation. Rule-based,
  so it can never invent words you didn't say.

## How it works

```
Ctrl+Win ──► keyboard hook ──► always-on mic ring (400 ms look-back, so your first
                                word is never clipped)
                                     │  release
                                     ▼
                          local ASR (Parakeet, CPU) ──► deterministic cleanup
                                     │
                                     ▼
                    injected at the cursor (route chosen per target app)
```

Rust throughout, with every Win32 call confined to one crate behind a trait wall — enforced in CI,
which is also what makes a macOS port a swap rather than a rewrite.

## Contributing

Issues and PRs welcome. `cargo test --workspace` and `cargo run -p xtask -- check-layering` must
pass; CI runs fmt, clippy (`-D warnings`), tests, and the layering check.

## License

[MIT](LICENSE). Third-party components and the speech model's required attribution are listed in
[THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).
