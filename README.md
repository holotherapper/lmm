# lmm

[![CI](https://github.com/holotherapper/lmm/actions/workflows/ci.yml/badge.svg)](https://github.com/holotherapper/lmm/actions)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)
[![Rust](https://img.shields.io/badge/rust-1.87%2B-orange.svg)](https://www.rust-lang.org)
[![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux-lightgrey.svg)]()

Local AI model manager for macOS and Linux.

**Download once. Use everywhere. Remove cleanly.**

<p align="center">
  <img src="assets/demo.gif" alt="lmm demo" width="640">
</p>

## Why lmm?

Local AI tools (LM Studio, Ollama, llama.cpp, ComfyUI, …) each maintain their own model directories. The same 8 GB model downloaded through different tools can consume 16–24 GB of disk.

`lmm` uses HF Cache as the single canonical store and exposes models to each tool via symlinks — **zero duplication, instant availability**.

## Features

- **Interactive search** — browse and install Hugging Face models from the terminal
- **Format support** — MLX, GGUF, safetensors
- **Tool adapters** — LM Studio, llama.cpp, Jan, ComfyUI, text-generation-webui, and more
- **Adopt existing models** — track models already in your HF Cache without re-downloading
- **Doctor & GC** — validate integrity, consolidate external models, reclaim disk space
- **Shell completions** — bash, zsh, fish, elvish, powershell

## Install

**macOS:**

```sh
brew install holotherapper/tap/lmm
```

**From source (macOS / Linux, requires Rust 1.87+):**

```sh
cargo install --git https://github.com/holotherapper/lmm
```

## Quick Start

```sh
lmm search                                          # interactive search → select → install
lmm add mlx-community/Qwen3-8B-4bit                 # install from Hugging Face
lmm add mlx-community/Qwen3-8B-4bit --tool lmstudio # install + expose to LM Studio
lmm list                                             # see all local models (with repo column)
lmm adopt                                           # track existing HF Cache models
lmm doctor                                          # validate + consolidate external models
lmm remove qwen3-8b-4bit-mlx                        # remove and reclaim disk space
lmm remove mlx-community/Qwen3-8B-4bit              # remove by repo name
lmm remove gemma-3-1b-it-qat-4bit                   # remove external models directly
```

## Commands

| Command | Description |
|---------|-------------|
| `lmm add <repo>` | Install a model and expose to tools. Aliases: `a`, `i`, `install` |
| `lmm remove [names]` | Remove tracked, external, or untracked models. Alias: `rm` |
| `lmm list` | List all local models with repo source. Alias: `ls` |
| `lmm info <name>` | Show details for matching models (accepts `org/repo`) |
| `lmm search [query]` | Search Hugging Face. Aliases: `find`, `discover` |
| `lmm adopt [names]` | Track unmanaged HF Cache models |
| `lmm update [names]` | Update models to latest revision. Alias: `upgrade` |
| `lmm doctor` | Validate cache, exposures, and external models |
| `lmm gc` | Clean temp files, stale entries, orphan blobs (`--dry-run` for preview) |
| `lmm config get/set/path` | Read or update configuration |
| `lmm completions <shell>` | Generate shell completions |

Run `lmm <command> --help` for full options.

### Key options for `add`

| Option | Description |
|--------|-------------|
| `--file <PATTERN>` | Wildcard for GGUF selection (e.g. `*Q4_K_M*`) |
| `--tool <T1,T2>` | Target tools (comma-separated) |
| `--format <F>` | `auto`, `mlx`, `gguf`, or `safetensors` |
| `--all` | Expose to all compatible tools |
| `--dry-run` | Preview without changes |
| `--replace` | Replace existing exposure |
| `-y, --yes` | Skip confirmation |

### Deletion rules for `remove`

Names can be aliases (`qwen3-8b-4bit-mlx`) or repo identifiers (`mlx-community/Qwen3-8B-4bit`).

| Target | Behavior |
|--------|----------|
| Tracked (managed) | Deletes exposure + HF Cache blobs |
| Tracked (adopted) | Deletes exposure only (add `--purge-cache` for HF Cache blobs) |
| External (lmstudio, jan, …) | Deletes files from the tool directory |
| External (ollama) | Delegates to `ollama rm` |
| Untracked HF Cache | Deletes the repo cache directory (specify by `org/repo`) |

## Tool Adapters

### Symlink / reference adapters (`--tool`)

| Tool | ID | Formats | Status |
|------|----|---------|--------|
| LM Studio | `lmstudio` | MLX, GGUF, safetensors | stable |
| Jan | `jan` | GGUF | experimental |
| llama.cpp | `llama-cpp` | GGUF | stable |
| ComfyUI | `comfyui` | safetensors, MLX, GGUF | experimental |
| AUTOMATIC1111 / Forge | `a1111` | safetensors, GGUF | experimental |
| InvokeAI | `invokeai` | safetensors, GGUF | experimental |
| Fooocus | `fooocus` | safetensors, GGUF | experimental |
| text-generation-webui | `text-gen-webui` | MLX, GGUF, safetensors | experimental |
| GPT4All | `gpt4all` | GGUF | experimental |

Experimental adapters appear in the TUI when their path is configured (`lmm config set paths.comfyui /path/to/ComfyUI/models`).

### HF Cache direct (no action needed)

These tools read HF Cache directly — `lmm add` makes models instantly available:

mlx-lm, transformers, mflux, Diffusers, mlx-whisper, faster-whisper, Kokoro, Bark, F5-TTS, MeloTTS, sentence-transformers, AudioCraft, vLLM, KoboldCpp, Tortoise TTS

### Display only

| Tool | Reason |
|------|--------|
| Ollama | Copies to blob store, doubling disk usage. Shown in `lmm list`; removable via `lmm remove` (delegates to `ollama rm`) |
| Draw Things | App Sandbox + proprietary format. Not manageable |

## Configuration

Config file: `~/Library/Application Support/dev.local.lmm/config.json` (macOS) or `~/.local/share/lmm/config.json` (Linux). Override with `LMM_STATE_DIR`.

```sh
lmm config set paths.comfyui /path/to/ComfyUI/models
lmm config set defaults.format gguf
lmm config get paths.lmstudio
lmm config path
```

Run `lmm config set` to see available keys.

## Environment Variables

| Variable | Description |
|----------|-------------|
| `LMM_STATE_DIR` | Override state directory |
| `HUGGINGFACE_HUB_CACHE` | Override HF Cache directory |
| `HF_TOKEN` | Hugging Face API token (for gated/private models) |
| `NO_COLOR` | Disable color output |

## Safety

- **Adopted** models keep HF Cache files unless `--purge-cache` is explicitly used
- **Exposure removal** validates paths are under the tool's configured root before deletion
- **`--dry-run`** previews any destructive operation before execution
- **Symlink exposures** are created atomically (stage → rename)

## License

Dual-licensed under either of:

- [MIT license](LICENSE-MIT)
- [Apache License 2.0](LICENSE-APACHE)

at your option. SPDX identifier: `MIT OR Apache-2.0`.
