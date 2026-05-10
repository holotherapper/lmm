# Changelog

## [0.2.0] - 2026-05-11

### Added
- `lmm remove` directly deletes external models (lmstudio, jan, comfyui via filesystem; ollama via `ollama rm`)
- `lmm remove` accepts `org/repo` identifiers to target a specific model when aliases collide
- `lmm remove` can delete untracked HF Cache entries by repo name
- `lmm list` default view includes a REPO column
- `lmm info` shows all matching models when multiple share the same alias
- `lmm info` accepts `org/repo` identifiers
- `lmm gc --dry-run` flag for CLI consistency with `remove` and `update`
- Interactive TUI selection in `lmm remove` includes external and untracked entries

### Fixed
- `lmm remove --purge-cache` no longer leaves empty repo cache stub directories
- Empty subdirectories within a snapshot (e.g. `voices/`) are cleaned up after file deletion

## [0.1.0] - 2026-05-01

### Added
- Model search with interactive TUI (`lmm search`)
- Install from Hugging Face with tool exposure (`lmm add`)
- Remove models and reclaim disk space (`lmm remove`)
- List all local models with format/tool filters (`lmm list`)
- Adopt untracked HF Cache models (`lmm adopt`)
- Update tracked models to latest revision (`lmm update`)
- Health check and consolidation (`lmm doctor`)
- Garbage collection (`lmm gc`)
- Shell completions (`lmm completions`)
- Tool adapters: LM Studio, Ollama, llama.cpp, Jan, ComfyUI, A1111, InvokeAI, Fooocus, text-generation-webui, GPT4All
- HF Cache as canonical model store with symlink deduplication
- External model consolidation (move tool-owned files into HF Cache)
- Config management (`lmm config get/set/path`)
