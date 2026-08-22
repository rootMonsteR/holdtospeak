# Third-party notices

This product bundles or downloads third-party software and a machine-learning model. Their
licenses and required attributions are reproduced below.

Everything here is **permissive** (MIT / Apache-2.0 / BSD / ISC / Unicode / CC-BY). There is no
copyleft (GPL/LGPL/AGPL) code in the shipped binaries — see *Deliberate exclusion* at the end,
which documents a real trap we hit.

---

## 1. Speech recognition model — **attribution required**

### Parakeet TDT 0.6B v2 (English), int8

* **Copyright © NVIDIA Corporation**
* **License: [Creative Commons Attribution 4.0 International (CC-BY-4.0)](https://creativecommons.org/licenses/by/4.0/)**
* Source model: [`nvidia/parakeet-tdt-0.6b-v2`](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v2)
* Distributed as an ONNX export by the [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx)
  project, downloaded from its `asr-models` release.

**Modification notice (required by CC-BY-4.0 §3(a)(1)(B)):** the model as used here has been
**modified** from NVIDIA's original release — it was converted to ONNX and quantized to int8 by
the sherpa-onnx project. This application does not further modify the model weights.

This application **does not redistribute** the model: it is downloaded on first run directly from
the upstream sherpa-onnx release, verified against a pinned SHA-256, and stored locally.

---

## 2. Native libraries (shipped alongside the executable)

| Component | License | Notes |
|---|---|---|
| [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx) (`sherpa-onnx-c-api.dll`) | Apache-2.0 | Speech-recognition runtime. We ship the **`no-tts`** build (see below). |
| [ONNX Runtime](https://github.com/microsoft/onnxruntime) (`onnxruntime.dll`) | MIT | Neural-network inference engine, © Microsoft Corporation. |

A copy of the Apache License 2.0 is available at <https://www.apache.org/licenses/LICENSE-2.0>.

---

## 3. Rust dependencies

The application links 105 third-party Rust crates. Their licenses:

| License | Crates |
|---|---|
| MIT OR Apache-2.0 (incl. equivalent spellings) | 66 |
| Unicode-3.0 | 18 |
| MIT | 6 |
| Apache-2.0 | 4 |
| ISC / Apache-2.0 AND ISC / Apache-2.0 OR ISC OR MIT | 4 |
| CDLA-Permissive-2.0 | 2 |
| BSD-3-Clause | 1 |
| 0BSD OR MIT OR Apache-2.0 | 1 |
| Unlicense OR MIT | 1 |
| MIT OR Zlib OR Apache-2.0 | 1 |
| (MIT OR Apache-2.0) AND Unicode-3.0 | 1 |

Notable direct dependencies: `windows` (MIT/Apache-2.0, © Microsoft), `cpal`, `rubato`, `rustfft`,
`hound`, `ureq`, `sha2`, `tar`, `bzip2`, `sherpa-onnx`.

To regenerate a complete, per-crate list with full license texts:

```
cargo install cargo-about
cargo about generate about.hbs > THIRD-PARTY-FULL.html
```

---

## Deliberate exclusion: espeak-ng (GPLv3)

sherpa-onnx's **default** prebuilt Windows binary statically links
[espeak-ng](https://github.com/espeak-ng/espeak-ng) (GPLv3) for text-to-speech, which this project
does not use. Shipping it would impose GPL obligations on the whole distribution.

We therefore build against the upstream **`-no-tts-`** archive, which excludes espeak-ng. This is
enforced mechanically, not by convention:

* `scripts/fetch-sherpa.ps1` downloads the no-tts archive and **fails the build** if espeak-ng
  markers (`ESPEAK_DATA_PATH`, `phonemize_eSpeak`, …) are found in the binary,
* `.cargo/config.toml` points `SHERPA_ONNX_LIB_DIR` at those vendored libraries so the build
  cannot silently fall back to the default archive,
* CI runs the same check on every push.

**General principle:** a permissive *source* license does not guarantee a permissive *binary*.
Prebuilt artifacts must be checked for what they statically link.
