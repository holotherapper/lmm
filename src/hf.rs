//! Hugging Face Hub API client: metadata, file trees, downloads, and search.
use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};

use reqwest::StatusCode;
use reqwest::blocking::{Client, RequestBuilder, Response};
use serde::Deserialize;

use crate::artifacts::CandidateFile;
use crate::error::{AppError, Result};
use crate::security::{
    repo_cache_dir_name, safe_join, validate_relative_repo_path, validate_repo_id,
};

#[derive(Clone, Debug)]
pub struct HfClient {
    endpoint: String,
    cache_dir: PathBuf,
    client: Client,
    token: Option<String>,
}

#[derive(Clone, Debug)]
pub struct RepoMetadata {
    pub commit: String,
    pub library_name: Option<String>,
}

#[derive(Clone, Debug)]
pub struct RepoFile {
    pub path: String,
    pub size_bytes: u64,
    pub oid: String,
}

#[derive(Clone, Debug)]
pub struct ModelSummary {
    pub id: String,
    pub downloads: Option<u64>,
    pub likes: Option<u64>,
    pub sha: Option<String>,
    pub tags: Vec<String>,
    pub pipeline_tag: Option<String>,
    pub library_name: Option<String>,
    pub model_type: Option<String>,
    pub architectures: Vec<String>,
    pub quant_bits: Option<u32>,
}

impl ModelSummary {
    pub fn formats(&self) -> Vec<&str> {
        let mut fmts = Vec::new();
        if self.tags.iter().any(|t| t == "gguf") {
            fmts.push("GGUF");
        }
        if self.tags.iter().any(|t| t == "mlx") {
            fmts.push("MLX");
        }
        if fmts.is_empty() && self.tags.iter().any(|t| t == "safetensors") {
            fmts.push("safetensors");
        }
        fmts
    }

    pub fn quant(&self) -> Option<&str> {
        for tag in &self.tags {
            match tag.as_str() {
                "2-bit" => return Some("2bit"),
                "3-bit" => return Some("3bit"),
                "4-bit" => return Some("4bit"),
                "5-bit" => return Some("5bit"),
                "6-bit" => return Some("6bit"),
                "8-bit" => return Some("8bit"),
                "16-bit" | "bf16" | "fp16" => return Some("fp16"),
                _ => {}
            }
        }
        None
    }
}

#[derive(Debug, Deserialize)]
struct ModelResponse {
    sha: String,
    library_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TreeEntry {
    #[serde(rename = "type")]
    kind: String,
    path: String,
    size: Option<u64>,
    oid: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ModelSummaryResponse {
    id: String,
    downloads: Option<u64>,
    likes: Option<u64>,
    sha: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    pipeline_tag: Option<String>,
    library_name: Option<String>,
    #[serde(rename = "createdAt")]
    #[allow(dead_code)]
    created_at: Option<String>,
    #[serde(default)]
    config: Option<ModelConfig>,
}

#[derive(Debug, Deserialize)]
struct ModelConfig {
    #[serde(default)]
    architectures: Vec<String>,
    model_type: Option<String>,
    quantization_config: Option<QuantConfig>,
}

#[derive(Debug, Deserialize)]
struct QuantConfig {
    bits: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct ModelDetailResponse {
    #[serde(default)]
    siblings: Vec<SiblingEntry>,
}

#[derive(Debug, Deserialize)]
struct SiblingEntry {
    #[serde(default)]
    #[expect(dead_code, reason = "required for JSON deserialization")]
    rfilename: String,
    #[serde(default)]
    size: Option<u64>,
}

impl HfClient {
    pub fn new(endpoint: String, cache_dir: PathBuf) -> Self {
        use std::time::Duration;
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(3600))
            .build()
            .unwrap_or_else(|_| Client::new());
        Self {
            endpoint: endpoint.trim_end_matches('/').to_string(),
            cache_dir,
            client,
            token: hf_token_from_env(),
        }
    }

    pub fn snapshot_path(&self, repo: &str, commit: &str) -> Result<PathBuf> {
        snapshot_path(&self.cache_dir, repo, commit)
    }

    pub fn blob_path(&self, repo: &str, oid: &str) -> Result<PathBuf> {
        blob_path(&self.cache_dir, repo, oid)
    }

    pub fn repo_metadata(&self, repo: &str, revision: &str) -> Result<RepoMetadata> {
        validate_repo_id(repo)?;
        let url = format!(
            "{}/api/models/{repo}/revision/{}",
            self.endpoint,
            encode_path_segment(revision)
        );
        let response =
            checked_response(self.get(url).send()?, &format!("model metadata `{repo}`"))?;
        let metadata = response.json::<ModelResponse>()?;
        Ok(RepoMetadata {
            commit: metadata.sha,
            library_name: metadata.library_name,
        })
    }

    pub fn repo_files(&self, repo: &str, commit: &str) -> Result<Vec<RepoFile>> {
        validate_repo_id(repo)?;
        let url = format!(
            "{}/api/models/{repo}/tree/{}?recursive=1&expand=1",
            self.endpoint,
            encode_path_segment(commit)
        );
        let response = checked_response(self.get(url).send()?, &format!("model tree `{repo}`"))?;
        let entries = response.json::<Vec<TreeEntry>>()?;
        entries
            .into_iter()
            .filter(|entry| entry.kind == "file")
            .map(|entry| {
                validate_relative_repo_path(&entry.path)?;
                let Some(size) = entry.size else {
                    return Err(AppError::InvalidInput(format!(
                        "missing size for HF file `{}`",
                        entry.path
                    )));
                };
                let Some(oid) = entry.oid else {
                    return Err(AppError::InvalidInput(format!(
                        "missing oid for HF file `{}`",
                        entry.path
                    )));
                };
                Ok(RepoFile {
                    path: entry.path,
                    size_bytes: size,
                    oid,
                })
            })
            .collect()
    }

    pub fn download_file(
        &self,
        repo: &str,
        commit: &str,
        file: &RepoFile,
        _tmp_dir: &Path,
    ) -> Result<PathBuf> {
        validate_repo_id(repo)?;
        validate_relative_repo_path(&file.path)?;

        let blob_path = self.blob_path(repo, &file.oid)?;
        if blob_path.exists()
            && blob_path
                .metadata()
                .map(|metadata| metadata.len())
                .unwrap_or(0)
                == file.size_bytes
            && verify_blob_hash(&blob_path, &file.oid, file.size_bytes)
        {
            return Ok(blob_path);
        }

        if let Some(parent) = blob_path.parent() {
            fs::create_dir_all(parent).map_err(|source| AppError::CreateDir {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let tmp_path = blob_path.with_file_name(format!("{}.{}.tmp", file.oid, std::process::id()));
        let url = format!(
            "{}/{repo}/resolve/{}/{}",
            self.endpoint,
            encode_path_segment(commit),
            encode_path_segment(&file.path)
        );

        let mut response = checked_response(
            self.get(url).send()?,
            &format!("file `{}` from `{repo}`", file.path),
        )?;
        let mut tmp = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp_path)
            .map_err(|source| AppError::Write {
                path: tmp_path.clone(),
                source,
            })?;
        let max_bytes = file.size_bytes + (1024 * 1024);
        let bytes_written =
            io::copy(&mut response.by_ref().take(max_bytes), &mut tmp).map_err(|source| {
                AppError::Write {
                    path: tmp_path.clone(),
                    source,
                }
            })?;
        if bytes_written != file.size_bytes {
            drop(tmp);
            let _ = fs::remove_file(&tmp_path);
            return Err(AppError::InvalidInput(format!(
                "downloaded size mismatch for `{}`: expected {}, got {}",
                file.path, file.size_bytes, bytes_written
            )));
        }

        tmp.sync_all().map_err(|source| AppError::Write {
            path: tmp_path.clone(),
            source,
        })?;
        drop(tmp);

        if !verify_blob_hash(&tmp_path, &file.oid, file.size_bytes) {
            let _ = fs::remove_file(&tmp_path);
            return Err(AppError::InvalidInput(format!(
                "hash mismatch for downloaded file `{}`: oid {}",
                file.path, file.oid
            )));
        }

        fs::rename(&tmp_path, &blob_path).map_err(|source| AppError::Rename {
            from: tmp_path,
            to: blob_path.clone(),
            source,
        })?;
        Ok(blob_path)
    }

    pub fn search_models(
        &self,
        query: &str,
        author: Option<&str>,
        limit: usize,
        sort: &str,
    ) -> Result<Vec<ModelSummary>> {
        let url = format!("{}/api/models", self.endpoint);
        let mut request = self.get(url).query(&[
            ("search", query.to_string()),
            ("limit", limit.to_string()),
            ("sort", sort.to_string()),
            ("config", "true".to_string()),
        ]);
        if let Some(author) = author {
            request = request.query(&[("author", author)]);
        }
        let response = checked_response(request.send()?, "model search")?;
        let models = response.json::<Vec<ModelSummaryResponse>>()?;
        Ok(models
            .into_iter()
            .map(|model| {
                let (model_type, architectures, quant_bits) = match model.config {
                    Some(config) => (
                        config.model_type,
                        config.architectures,
                        config.quantization_config.and_then(|q| q.bits),
                    ),
                    None => (None, Vec::new(), None),
                };
                ModelSummary {
                    id: model.id,
                    downloads: model.downloads,
                    likes: model.likes,
                    sha: model.sha,
                    tags: model.tags,
                    pipeline_tag: model.pipeline_tag,
                    library_name: model.library_name,
                    model_type,
                    architectures,
                    quant_bits,
                }
            })
            .collect())
    }

    pub fn model_total_size(&self, repo: &str) -> Result<u64> {
        validate_repo_id(repo)?;
        let url = format!("{}/api/models/{repo}?blobs=true", self.endpoint);
        let response = checked_response(self.get(url).send()?, &format!("model size `{repo}`"))?;
        let detail = response.json::<ModelDetailResponse>()?;
        Ok(detail.siblings.iter().filter_map(|s| s.size).sum())
    }

    fn get(&self, url: String) -> RequestBuilder {
        let request = self.client.get(url);
        if let Some(token) = &self.token
            && self.endpoint.starts_with("https://")
        {
            request.bearer_auth(token)
        } else {
            request
        }
    }
}

fn encode_path_segment(s: &str) -> String {
    s.bytes()
        .flat_map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                vec![b as char]
            }
            _ => format!("%{b:02X}").chars().collect(),
        })
        .collect()
}

fn hf_token_from_env() -> Option<String> {
    env::var("HF_TOKEN")
        .ok()
        .or_else(|| env::var("HUGGING_FACE_HUB_TOKEN").ok())
        .filter(|token| !token.trim().is_empty())
}

fn checked_response(response: Response, context: &str) -> Result<Response> {
    let status = response.status();
    if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
        return Err(AppError::InvalidInput(format!(
            "Hugging Face requires authentication for {context}. Public models do not require an account; set HF_TOKEN only for private or gated repositories."
        )));
    }
    if status == StatusCode::NOT_FOUND {
        return Err(AppError::InvalidInput(format!(
            "Hugging Face resource not found for {context}"
        )));
    }
    if status == StatusCode::TOO_MANY_REQUESTS {
        return Err(AppError::InvalidInput(format!(
            "Hugging Face rate limit exceeded for {context}. Please wait a moment and try again."
        )));
    }
    if status.is_server_error() {
        return Err(AppError::InvalidInput(format!(
            "Hugging Face server error ({status}) for {context}. The service may be temporarily unavailable — please try again later."
        )));
    }
    Ok(response.error_for_status()?)
}

pub fn snapshot_path(cache_dir: &Path, repo: &str, commit: &str) -> Result<PathBuf> {
    if commit.contains('/') || commit.contains("..") {
        return Err(AppError::InvalidInput(format!(
            "path traversal in commit: {commit}"
        )));
    }
    Ok(cache_dir
        .join(repo_cache_dir_name(repo))
        .join("snapshots")
        .join(commit))
}

pub fn blob_path(cache_dir: &Path, repo: &str, oid: &str) -> Result<PathBuf> {
    if oid.contains('/') || oid.contains("..") {
        return Err(AppError::InvalidInput(format!(
            "path traversal in oid: {oid}"
        )));
    }
    Ok(cache_dir
        .join(repo_cache_dir_name(repo))
        .join("blobs")
        .join(oid))
}

pub fn ensure_snapshot_link(
    cache_dir: &Path,
    repo: &str,
    commit: &str,
    file: &RepoFile,
) -> Result<PathBuf> {
    let snapshot_root = snapshot_path(cache_dir, repo, commit)?;
    let target_path = safe_join(&snapshot_root, &file.path)?;
    if let Some(parent) = target_path.parent() {
        fs::create_dir_all(parent).map_err(|source| AppError::CreateDir {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    let blob = blob_path(cache_dir, repo, &file.oid)?;
    if target_path.symlink_metadata().is_ok() {
        if links_to_blob(&target_path, &blob)? {
            return Ok(target_path);
        }
        // Symlink exists but points to a different blob. This can happen when
        // another tool (e.g. huggingface_hub, mlx_lm) created the snapshot with
        // a different blob naming scheme. If the target file exists and has the
        // expected size, accept it rather than failing.
        if target_path.exists()
            && target_path.metadata().map(|m| m.len()).unwrap_or(0) == file.size_bytes
            && verify_blob_hash(&target_path, &file.oid, file.size_bytes)
        {
            return Ok(target_path);
        }
        return Err(AppError::InvalidInput(format!(
            "snapshot path already exists but does not point to the expected blob: {}",
            target_path.display()
        )));
    }

    create_symlink(&blob, &target_path)?;
    Ok(target_path)
}

pub fn candidate_files(files: &[RepoFile]) -> Vec<CandidateFile> {
    files
        .iter()
        .map(|file| CandidateFile {
            path: file.path.clone(),
            size_bytes: file.size_bytes,
        })
        .collect()
}

fn verify_blob_hash(path: &Path, oid: &str, expected_size: u64) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if metadata.len() != expected_size {
        return false;
    }
    let is_lfs = oid.len() == 64 && oid.chars().all(|c| c.is_ascii_hexdigit());
    let is_git_sha1 = oid.len() == 40 && oid.chars().all(|c| c.is_ascii_hexdigit());
    if is_git_sha1 && expected_size > 1024 * 1024 {
        return true;
    }
    let computed = if is_lfs {
        compute_sha256(path)
    } else {
        compute_git_sha1(path, metadata.len())
    };
    let Ok(hash) = computed else {
        return false;
    };
    hash == oid
}

fn compute_sha256(path: &Path) -> std::io::Result<String> {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    let mut file = fs::File::open(path)?;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = io::Read::read(&mut file, &mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn compute_git_sha1(path: &Path, size: u64) -> std::io::Result<String> {
    use sha1::Digest;
    let mut hasher = sha1::Sha1::new();
    hasher.update(format!("blob {size}\0").as_bytes());
    let mut file = fs::File::open(path)?;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = io::Read::read(&mut file, &mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(unix)]
fn create_symlink(source: &Path, target: &Path) -> Result<()> {
    std::os::unix::fs::symlink(source, target).map_err(|source_error| AppError::Write {
        path: target.to_path_buf(),
        source: source_error,
    })
}

fn links_to_blob(target: &Path, blob: &Path) -> Result<bool> {
    let target = fs::canonicalize(target).map_err(|source| AppError::Read {
        path: target.to_path_buf(),
        source,
    })?;
    let blob = fs::canonicalize(blob).map_err(|source| AppError::Read {
        path: blob.to_path_buf(),
        source,
    })?;
    Ok(target == blob)
}

#[cfg(not(unix))]
fn create_symlink(_source: &Path, target: &Path) -> Result<()> {
    Err(AppError::InvalidInput(format!(
        "symlink creation is unsupported on this platform for `{}`",
        target.display()
    )))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn model_summary_formats_should_report_known_artifact_tags() {
        let summary = model_summary_with_tags(["gguf", "mlx", "text-generation"]);

        assert_eq!(summary.formats(), vec!["GGUF", "MLX"]);
    }

    #[test]
    fn model_summary_formats_should_fall_back_to_safetensors_tag() {
        let summary = model_summary_with_tags(["safetensors"]);

        assert_eq!(summary.formats(), vec!["safetensors"]);
    }

    #[test]
    fn model_summary_quant_should_normalize_bit_tags() {
        let summary = model_summary_with_tags(["4-bit"]);

        assert_eq!(summary.quant(), Some("4bit"));
    }

    #[test]
    fn snapshot_path_should_use_huggingface_cache_layout() {
        let path = snapshot_path(Path::new("/cache"), "org/repo", "abc123").unwrap();

        assert_eq!(
            path,
            Path::new("/cache")
                .join("models--org--repo")
                .join("snapshots")
                .join("abc123")
        );
    }

    #[test]
    fn blob_path_should_use_huggingface_cache_layout() {
        let path = blob_path(Path::new("/cache"), "org/repo", "deadbeef").unwrap();

        assert_eq!(
            path,
            Path::new("/cache")
                .join("models--org--repo")
                .join("blobs")
                .join("deadbeef")
        );
    }

    #[test]
    fn candidate_files_should_preserve_repo_file_paths_and_sizes() {
        let files = vec![RepoFile {
            path: "nested/model.gguf".to_string(),
            size_bytes: 42,
            oid: "oid".to_string(),
        }];

        assert_eq!(
            candidate_files(&files),
            vec![CandidateFile {
                path: "nested/model.gguf".to_string(),
                size_bytes: 42,
            }]
        );
    }

    #[cfg(unix)]
    #[test]
    fn ensure_snapshot_link_should_create_parent_dirs_and_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let cache_dir = dir.path();
        let file = repo_file("nested/model.gguf", 5, "blob-a");
        let blob = blob_path(cache_dir, "org/repo", &file.oid).unwrap();
        fs::create_dir_all(blob.parent().unwrap()).unwrap();
        fs::write(&blob, b"model").unwrap();

        let linked = ensure_snapshot_link(cache_dir, "org/repo", "commit-a", &file).unwrap();

        assert_eq!(
            fs::canonicalize(linked).unwrap(),
            fs::canonicalize(blob).unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn ensure_snapshot_link_should_accept_existing_file_with_matching_hash() {
        let dir = tempfile::tempdir().unwrap();
        let cache_dir = dir.path();
        let file = repo_file("model.gguf", 5, "8da8c246a5cd72d1659cb6558111d901eeed768b");
        let blob = blob_path(cache_dir, "org/repo", &file.oid).unwrap();
        let target = snapshot_path(cache_dir, "org/repo", "commit-a")
            .unwrap()
            .join(&file.path);
        fs::create_dir_all(blob.parent().unwrap()).unwrap();
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&blob, b"other").unwrap();
        fs::write(&target, b"model").unwrap();

        let linked = ensure_snapshot_link(cache_dir, "org/repo", "commit-a", &file).unwrap();

        assert_eq!(linked, target);
    }

    #[cfg(unix)]
    #[test]
    fn ensure_snapshot_link_should_reject_existing_file_with_wrong_size() {
        let dir = tempfile::tempdir().unwrap();
        let cache_dir = dir.path();
        let file = repo_file("model.gguf", 5, "blob-a");
        let blob = blob_path(cache_dir, "org/repo", &file.oid).unwrap();
        let target = snapshot_path(cache_dir, "org/repo", "commit-a")
            .unwrap()
            .join(&file.path);
        fs::create_dir_all(blob.parent().unwrap()).unwrap();
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(&blob, b"other").unwrap();
        fs::write(&target, b"bad").unwrap();

        let error = ensure_snapshot_link(cache_dir, "org/repo", "commit-a", &file).unwrap_err();

        assert!(matches!(error, AppError::InvalidInput(_)));
    }

    fn repo_file(path: &str, size_bytes: u64, oid: &str) -> RepoFile {
        RepoFile {
            path: path.to_string(),
            size_bytes,
            oid: oid.to_string(),
        }
    }

    fn model_summary_with_tags<const N: usize>(tags: [&str; N]) -> ModelSummary {
        ModelSummary {
            id: "org/repo".to_string(),
            downloads: None,
            likes: None,
            sha: None,
            tags: tags.into_iter().map(str::to_string).collect(),
            pipeline_tag: None,
            library_name: None,
            model_type: None,
            architectures: Vec::new(),
            quant_bits: None,
        }
    }
}
