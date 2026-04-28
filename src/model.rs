//! Domain types for tracked models, artifacts, sources, and exposures.
use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FormatKind {
    Mlx,
    Gguf,
    GgufSplit,
    Safetensors,
    #[serde(other)]
    Unknown,
}

impl fmt::Display for FormatKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Mlx => f.write_str("mlx"),
            Self::Gguf => f.write_str("gguf"),
            Self::GgufSplit => f.write_str("gguf-split"),
            Self::Safetensors => f.write_str("safetensors"),
            Self::Unknown => f.write_str("unknown"),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Ownership {
    Managed,
    Adopted,
    OwnedAdopted,
    External,
}

impl Ownership {
    pub fn deletes_canonical_bytes_by_default(&self) -> bool {
        matches!(self, Self::Managed | Self::OwnedAdopted)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExposureStatus {
    Ok,
    Cached,
    Adopted,
    External,
    Stale,
    Broken,
    Partial,
    Ambiguous,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Source {
    pub kind: String,
    pub repo: String,
    pub revision: String,
    pub commit: String,
    pub ownership: Ownership,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ArtifactFile {
    pub path: String,
    pub size_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hf_blob: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Artifact {
    pub signature: String,
    pub size_bytes: u64,
    pub full_snapshot: bool,
    pub files: Vec<ArtifactFile>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HfCacheLocation {
    pub snapshot_path: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Locations {
    pub hf_cache: Option<HfCacheLocation>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExposureEntry {
    pub status: ExposureStatus,
    pub strategy: String,
    pub path: Option<PathBuf>,
    pub created_by: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ModelEntry {
    pub name: String,
    pub source: Source,
    pub format: FormatKind,
    pub artifact: Artifact,
    pub locations: Locations,
    #[serde(default)]
    pub exposures: std::collections::BTreeMap<String, ExposureEntry>,
}

#[cfg(test)]
mod tests {
    use super::Ownership;

    #[test]
    fn deletion_policy_is_conservative_for_shared_cache_entries() {
        assert!(Ownership::Managed.deletes_canonical_bytes_by_default());
        assert!(Ownership::OwnedAdopted.deletes_canonical_bytes_by_default());
        assert!(!Ownership::Adopted.deletes_canonical_bytes_by_default());
        assert!(!Ownership::External.deletes_canonical_bytes_by_default());
    }
}
