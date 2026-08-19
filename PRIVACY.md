# Privacy

The short version: **your voice never leaves your machine.** Not to us, not to anyone. There is no
account, no telemetry, no analytics, no crash reporting, and no cloud service behind this app.

This document exists so you can verify that claim rather than trust it.

## What happens to your audio

1. The microphone is captured locally into a fixed-size in-memory ring buffer.
2. When you release the push-to-talk keys, that audio is written to a **single temporary WAV file**
   in your system temp directory, passed to a local speech-recognition process, and **deleted
   immediately afterwards**.
3. Recognition runs on your CPU, in a process on your machine, using a model stored on your disk.
4. The resulting text is placed at your cursor.

No step involves a network. Audio is never uploaded, never stored long-term, and never used to
train anything.

> **Known limitation, stated plainly:** step 2 writes audio to disk transiently. This is a
> prototype-era shortcut, not a design goal — the intended architecture passes audio to the
> recognizer through shared memory and never touches the filesystem. Until that lands, be aware
> the temp file exists for the duration of one transcription.

## The one time it uses the network

**Downloading the speech model, once.** The app cannot ship the model (it is ~460 MB and licensed
separately), so on first run it downloads it from the upstream sherpa-onnx GitHub release:

```
https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8.tar.bz2
```

The download announces itself in the console, is verified against a **pinned SHA-256** (a corrupted
or substituted archive is rejected and nothing is installed), and never happens again. If you would
rather it never touch the network at all, obtain the model separately and point the app at it with
`--model-dir` or the `NIB_ASR_MODEL_DIR` environment variable.

There is **no** update check, **no** license server, **no** usage ping.

## How to verify all of this yourself

* **Unplug the network cable** (after the model is installed) and use the app normally. Everything
  works.
* **Run a packet capture** (Wireshark, or `Resource Monitor → Network`) and watch the process. You
  will see no connections.
* **Read the code.** The only outbound HTTP in the entire codebase is in `crates/nib-models`, and
  it has exactly one hard-coded URL.
* **Block it in the firewall.** The app is designed to work fine with all network access denied
  once the model is present.

## What is stored on your machine

| Data | Location | Why |
|---|---|---|
| Speech model | `%LOCALAPPDATA%\HoldToSpeak\models\` | Recognition, offline |
| Personal dictionary | `%APPDATA%\HoldToSpeak\dictionary.txt` | Your jargon corrections (plain text — read/edit it freely) |
| Hotkey settings | `%APPDATA%\HoldToSpeak\hotkeys.toml` | Your key bindings (plain text) |

Delete those folders and the app is back to a clean state. Nothing is stored anywhere else.

## What the app deliberately refuses to do

* **Password fields.** When the focused control reports itself as a password field, dictation is
  **refused** rather than typed. This is checked live via UI Automation, not assumed.
* **Screenshots.** The app never captures your screen. It reads only the *name* of the focused
  application and whether the focused control is a text or password field — enough to choose how to
  insert text, and nothing more.
* **Reading your documents.** It does not read the contents of what you are typing into.
