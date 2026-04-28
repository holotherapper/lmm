# Changelog

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
