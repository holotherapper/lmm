//! Metadata for tools that read HF Cache directly.
//! These tools need no file operations — downloading to HF Cache is enough.
//! They are NOT ToolAdapter implementations; they only provide display metadata
//! for the "Also available via:" message after `lmm add`.
use crate::model::FormatKind;

pub struct DirectHfTool {
    pub id: &'static str,
    pub display_name: &'static str,
    pub formats: &'static [FormatKind],
}

const ALL_FORMATS: &[FormatKind] = &[FormatKind::Mlx, FormatKind::Safetensors, FormatKind::Gguf];
const MLX_FORMATS: &[FormatKind] = &[FormatKind::Mlx];
const SAFETENSORS_FORMATS: &[FormatKind] = &[FormatKind::Mlx, FormatKind::Safetensors];

pub const DIRECT_HF_TOOLS: &[DirectHfTool] = &[
    DirectHfTool {
        id: "mlx-lm",
        display_name: "mlx-lm",
        formats: MLX_FORMATS,
    },
    DirectHfTool {
        id: "transformers",
        display_name: "transformers",
        formats: SAFETENSORS_FORMATS,
    },
    DirectHfTool {
        id: "mflux",
        display_name: "mflux",
        formats: SAFETENSORS_FORMATS,
    },
    DirectHfTool {
        id: "diffusers",
        display_name: "Diffusers",
        formats: SAFETENSORS_FORMATS,
    },
    DirectHfTool {
        id: "mlx-whisper",
        display_name: "mlx-whisper",
        formats: ALL_FORMATS,
    },
    DirectHfTool {
        id: "faster-whisper",
        display_name: "faster-whisper",
        formats: SAFETENSORS_FORMATS,
    },
    DirectHfTool {
        id: "kokoro",
        display_name: "Kokoro",
        formats: SAFETENSORS_FORMATS,
    },
    DirectHfTool {
        id: "bark",
        display_name: "Bark",
        formats: SAFETENSORS_FORMATS,
    },
    DirectHfTool {
        id: "f5-tts",
        display_name: "F5-TTS",
        formats: SAFETENSORS_FORMATS,
    },
    DirectHfTool {
        id: "melotts",
        display_name: "MeloTTS",
        formats: SAFETENSORS_FORMATS,
    },
    DirectHfTool {
        id: "sentence-transformers",
        display_name: "sentence-transformers",
        formats: SAFETENSORS_FORMATS,
    },
    DirectHfTool {
        id: "audiocraft",
        display_name: "AudioCraft",
        formats: SAFETENSORS_FORMATS,
    },
    DirectHfTool {
        id: "vllm",
        display_name: "vLLM",
        formats: SAFETENSORS_FORMATS,
    },
    DirectHfTool {
        id: "koboldcpp",
        display_name: "KoboldCpp",
        formats: &[FormatKind::Gguf],
    },
    DirectHfTool {
        id: "tortoise-tts",
        display_name: "Tortoise TTS",
        formats: SAFETENSORS_FORMATS,
    },
];
