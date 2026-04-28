# lmm

[![CI](https://github.com/holotherapper/lmm/actions/workflows/ci.yml/badge.svg)](https://github.com/holotherapper/lmm/actions)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.86%2B-orange.svg)](https://www.rust-lang.org)

Local AI model manager for Apple Silicon.

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

Requires macOS (Apple Silicon).

```sh
brew tap holotherapper/tap
brew install lmm
```

Or build from source (requires Rust 1.86+):

```sh
cargo install --path .
```

## Quick Start

```sh
lmm search                                          # interactive search → select → install
lmm add mlx-community/Qwen3-8B-4bit                 # install from Hugging Face
lmm add mlx-community/Qwen3-8B-4bit --tool lmstudio # install + expose to LM Studio
lmm list                                             # see all local models
lmm adopt                                           # track existing HF Cache models
lmm doctor                                          # validate + consolidate external models
lmm remove qwen3-8b-4bit-mlx                        # remove and reclaim disk space
```

## Commands

| Command | Description |
|---------|-------------|
| `lmm add <repo>` | Install a model and expose to tools. Aliases: `a`, `i`, `install` |
| `lmm remove [names]` | Remove exposures and reclaim cache. Alias: `rm` |
| `lmm list` | List all local models. Alias: `ls` |
| `lmm info <name>` | Show details for one model |
| `lmm search [query]` | Search Hugging Face. Aliases: `find`, `discover` |
| `lmm adopt [names]` | Track unmanaged HF Cache models |
| `lmm update [names]` | Update models to latest revision. Alias: `upgrade` |
| `lmm doctor` | Validate cache, exposures, and external models |
| `lmm gc` | Clean temp files, stale entries, orphan blobs |
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

| Ownership | Default | With `--purge-cache` |
|-----------|---------|---------------------|
| Managed | Deletes exposure + HF Cache blobs | — |
| Adopted | Deletes exposure only | Also deletes HF Cache blobs |

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
| Ollama | Copies to blob store, doubling disk usage. Shown in `lmm list` for visibility |
| Draw Things | App Sandbox + proprietary format. Not manageable |

## Configuration

Config file: `~/Library/Application Support/dev.local.lmm/config.json` (override with `LMM_STATE_DIR`).

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

[Apache License 2.0](LICENSE)
