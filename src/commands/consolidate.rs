//! External model consolidation — move tool-owned files into HF Cache
//! and replace the originals with symlinks.
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use sha1::Digest as _;

use crate::artifacts::{CandidateFile, default_alias, detect_format};
use crate::config::Config;
use crate::error::{AppError, Result};
use crate::format;
use crate::hf::{HfClient, RepoFile, candidate_files};
use crate::lock::LockFile;
use crate::model::{
    Artifact, ArtifactFile, ExposureEntry, ExposureStatus, FormatKind, HfCacheLocation, Locations,
    ModelEntry, Ownership, Source,
};
use crate::paths::expand_tilde;
use crate::security::repo_cache_dir_name;
use crate::tui;

// ── Public types ───────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub(crate) struct ConsolidationCandidate {
    pub repo_id: String,
    pub tool: String,
    pub local_path: PathBuf,
    pub size_bytes: u64,
}

// ── Internal error type ────────────────────────────────────────────────

enum ConsolidateError {
    Skip(SkipReason),
    App(AppError),
}

enum SkipReason {
    RepoNotFound,
    HashMismatch(String),
}

impl From<AppError> for ConsolidateError {
    fn from(e: AppError) -> Self {
        if let AppError::InvalidInput(ref msg) = e
            && (msg.contains("not found") || msg.contains("authentication"))
        {
            return Self::Skip(SkipReason::RepoNotFound);
        }
        Self::App(e)
    }
}

// ── Discovery ──────────────────────────────────────────────────────────

pub(crate) fn discover_candidates(config: &Config) -> Vec<ConsolidationCandidate> {
    let mut candidates = Vec::new();
    if let Some(root) = config.paths.lmstudio.as_deref() {
        candidates.extend(discover_lmstudio_candidates(&expand_tilde(root)));
    }
    candidates
}

fn discover_lmstudio_candidates(root: &Path) -> Vec<ConsolidationCandidate> {
    let mut candidates = Vec::new();
    let Ok(authors) = fs::read_dir(root) else {
        return candidates;
    };
    for author_entry in authors.flatten() {
        let author_path = author_entry.path();
        if !author_path
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_dir())
        {
            continue;
        }
        let Some(author) = author_path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Ok(models) = fs::read_dir(&author_path) else {
            continue;
        };
        for model_entry in models.flatten() {
            let model_path = model_entry.path();
            if !model_path.is_dir()
                || model_path
                    .symlink_metadata()
                    .is_ok_and(|m| m.file_type().is_symlink())
            {
                continue;
            }
            let Some(model_name) = model_path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let size = dir_size(&model_path);
            if size == 0 {
                continue;
            }
            candidates.push(ConsolidationCandidate {
                repo_id: format!("{author}/{model_name}"),
                tool: "lmstudio".to_string(),
                local_path: model_path.clone(),
                size_bytes: size,
            });
        }
    }
    candidates
}

fn dir_size(path: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(path) else {
        return 0;
    };
    entries.flatten().fold(0, |acc, entry| {
        let Ok(meta) = entry.path().symlink_metadata() else {
            return acc;
        };
        if meta.file_type().is_symlink() {
            acc
        } else if meta.file_type().is_file() {
            acc + meta.len()
        } else if meta.file_type().is_dir() {
            acc + dir_size(&entry.path())
        } else {
            acc
        }
    })
}

// ── Interactive consolidation ──────────────────────────────────────────

pub(crate) fn consolidate_interactive(
    client: &HfClient,
    lock: &mut LockFile,
    candidates: &[ConsolidationCandidate],
    hf_cache: &Path,
) -> Result<()> {
    if candidates.is_empty() {
        return Ok(());
    }

    let items: Vec<(String, String, String)> = candidates
        .iter()
        .map(|c| {
            (
                c.repo_id.clone(),
                format!("{} · {}", c.repo_id, format::bytes(c.size_bytes)),
                c.tool.clone(),
            )
        })
        .collect();
    let initial: Vec<String> = candidates.iter().map(|c| c.repo_id.clone()).collect();

    let selected_ids = tui::select_many("Consolidate into HF Cache?", &items, &initial)?;

    if selected_ids.is_empty() {
        return Ok(());
    }

    let selected: Vec<&ConsolidationCandidate> = candidates
        .iter()
        .filter(|c| selected_ids.contains(&c.repo_id))
        .collect();

    let mut consolidated_size = 0_u64;
    let mut success_count = 0_usize;

    for candidate in &selected {
        match consolidate_one(client, candidate, hf_cache, lock) {
            Ok(size) => {
                consolidated_size += size;
                success_count += 1;
                format::status(
                    "\u{2713}",
                    &format!(
                        "{} ({})",
                        candidate.repo_id,
                        format::bytes(candidate.size_bytes)
                    ),
                );
            }
            Err(ConsolidateError::Skip(reason)) => {
                let reason_msg = match &reason {
                    SkipReason::RepoNotFound => "repo not found or requires auth".to_string(),
                    SkipReason::HashMismatch(file) => format!("hash mismatch: {file}"),
                };
                format::status(
                    "○",
                    &format::dim(&format!("{} — {reason_msg}", candidate.repo_id)),
                );
                eprintln!(
                    "  try: {}",
                    format::cyan(&format!(
                        "lmm add {} --tool {}",
                        candidate.repo_id, candidate.tool
                    ))
                );
            }
            Err(ConsolidateError::App(e)) => {
                format::status(
                    "!",
                    &format::yellow(&format!("{} — {e}", candidate.repo_id)),
                );
            }
        }
    }

    eprintln!();
    format::status(
        "\u{2713}",
        &format!(
            "Consolidated {success_count} model(s) (deduplicated {})",
            format::bytes(consolidated_size)
        ),
    );

    Ok(())
}

// ── Single-model consolidation ─────────────────────────────────────────

fn consolidate_one(
    client: &HfClient,
    candidate: &ConsolidationCandidate,
    hf_cache: &Path,
    lock: &mut LockFile,
) -> std::result::Result<u64, ConsolidateError> {
    let metadata = client.repo_metadata(&candidate.repo_id, "main")?;
    let repo_files = client.repo_files(&candidate.repo_id, &metadata.commit)?;
    let local_files = list_local_relative_paths(&candidate.local_path);
    let api_lookup: BTreeMap<&str, &RepoFile> =
        repo_files.iter().map(|f| (f.path.as_str(), f)).collect();

    verify_all_hashes(&candidate.local_path, &local_files, &api_lookup)?;

    let repo_cache = hf_cache.join(repo_cache_dir_name(&candidate.repo_id));
    let blobs_dir = repo_cache.join("blobs");
    let snapshot_dir = repo_cache.join("snapshots").join(&metadata.commit);
    create_dir(&blobs_dir)?;
    create_dir(&snapshot_dir)?;

    let unmapped: Vec<&str> = local_files
        .iter()
        .filter(|p| !api_lookup.contains_key(p.as_str()))
        .filter(|p| candidate.local_path.join(p).is_file())
        .map(String::as_str)
        .collect();
    if !unmapped.is_empty() {
        return Err(ConsolidateError::App(AppError::InvalidInput(format!(
            "local files not in HF repo (would be lost): {}",
            unmapped.join(", ")
        ))));
    }

    let (artifact_files, total_size) = move_files_to_cache(
        candidate,
        &local_files,
        &api_lookup,
        &blobs_dir,
        &snapshot_dir,
    )
    .map_err(|e| {
        if let ConsolidateError::App(ref inner) = e {
            eprintln!(
                "  {} consolidation partially failed for {}; files may remain in both {} and {}",
                crate::format::yellow("warning:"),
                candidate.repo_id,
                candidate.local_path.display(),
                blobs_dir.display(),
            );
            eprintln!("  original error: {inner}");
        }
        e
    })?;

    write_refs_main(&repo_cache, &metadata.commit)?;
    replace_dir_with_symlink(&candidate.local_path, &snapshot_dir)?;
    register_in_lock(
        candidate,
        &metadata,
        &repo_files,
        artifact_files,
        total_size,
        &snapshot_dir,
        lock,
    );

    Ok(total_size)
}

fn verify_all_hashes(
    base: &Path,
    local_files: &[String],
    api_lookup: &BTreeMap<&str, &RepoFile>,
) -> std::result::Result<(), ConsolidateError> {
    for rel_path in local_files {
        if let Some(api_file) = api_lookup.get(rel_path.as_str()) {
            verify_file_hash(&base.join(rel_path), api_file)?;
        }
    }
    Ok(())
}

fn move_files_to_cache(
    candidate: &ConsolidationCandidate,
    local_files: &[String],
    api_lookup: &BTreeMap<&str, &RepoFile>,
    blobs_dir: &Path,
    snapshot_dir: &Path,
) -> std::result::Result<(Vec<ArtifactFile>, u64), ConsolidateError> {
    let mut artifact_files = Vec::new();
    let mut total_size = 0_u64;

    for rel_path in local_files {
        let local_path = candidate.local_path.join(rel_path);
        if !local_path.is_file() {
            continue;
        }
        let Some(api_file) = api_lookup.get(rel_path.as_str()) else {
            continue;
        };

        move_to_blob(&local_path, blobs_dir, &api_file.oid, api_file.size_bytes)?;
        create_snapshot_link(snapshot_dir, rel_path, blobs_dir, &api_file.oid)?;

        artifact_files.push(ArtifactFile {
            path: rel_path.clone(),
            size_bytes: api_file.size_bytes,
            hf_blob: Some(api_file.oid.clone()),
            sha256: None,
            role: None,
        });
        total_size += api_file.size_bytes;
    }

    Ok((artifact_files, total_size))
}

fn move_to_blob(
    local_path: &Path,
    blobs_dir: &Path,
    oid: &str,
    expected_size: u64,
) -> std::result::Result<(), ConsolidateError> {
    validate_oid(oid)?;
    let blob_path = blobs_dir.join(oid);
    if blob_path.exists() {
        let existing_size = fs::metadata(&blob_path).map(|m| m.len()).unwrap_or(0);
        if existing_size != expected_size {
            return Err(ConsolidateError::Skip(SkipReason::HashMismatch(format!(
                "existing blob size mismatch for {oid}"
            ))));
        }
        let is_lfs = oid.len() == 64;
        let existing_hash = if is_lfs {
            compute_sha256(&blob_path)?
        } else {
            compute_git_sha1(&blob_path, existing_size)?
        };
        if existing_hash != oid {
            return Err(ConsolidateError::Skip(SkipReason::HashMismatch(format!(
                "existing blob hash mismatch for {oid}"
            ))));
        }
        if local_path.exists() {
            fs::remove_file(local_path).map_err(|source| {
                ConsolidateError::App(AppError::Write {
                    path: local_path.to_path_buf(),
                    source,
                })
            })?;
        }
    } else {
        fs::rename(local_path, &blob_path).map_err(|source| {
            ConsolidateError::App(AppError::Rename {
                from: local_path.to_path_buf(),
                to: blob_path,
                source,
            })
        })?;
    }
    Ok(())
}

fn create_snapshot_link(
    snapshot_dir: &Path,
    rel_path: &str,
    blobs_dir: &Path,
    oid: &str,
) -> std::result::Result<(), ConsolidateError> {
    validate_oid(oid)?;
    crate::security::validate_relative_repo_path(rel_path).map_err(ConsolidateError::App)?;
    let snapshot_file = snapshot_dir.join(rel_path);
    if snapshot_file.exists() {
        return Ok(());
    }
    if let Some(parent) = snapshot_file.parent() {
        create_dir(parent)?;
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(blobs_dir.join(oid), &snapshot_file).map_err(|source| {
        ConsolidateError::App(AppError::Write {
            path: snapshot_file,
            source,
        })
    })?;
    Ok(())
}

fn write_refs_main(repo_cache: &Path, commit: &str) -> std::result::Result<(), ConsolidateError> {
    let refs_dir = repo_cache.join("refs");
    create_dir(&refs_dir)?;
    let refs_main = refs_dir.join("main");
    fs::write(&refs_main, commit).map_err(|source| {
        ConsolidateError::App(AppError::Write {
            path: refs_main,
            source,
        })
    })?;
    Ok(())
}

fn replace_dir_with_symlink(
    dir: &Path,
    target: &Path,
) -> std::result::Result<(), ConsolidateError> {
    if !dir.exists()
        || dir
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink())
    {
        return Ok(());
    }
    fs::remove_dir_all(dir).map_err(|source| {
        ConsolidateError::App(AppError::Write {
            path: dir.to_path_buf(),
            source,
        })
    })?;
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, dir).map_err(|source| {
        ConsolidateError::App(AppError::Write {
            path: dir.to_path_buf(),
            source,
        })
    })?;
    Ok(())
}

fn register_in_lock(
    candidate: &ConsolidationCandidate,
    metadata: &crate::hf::RepoMetadata,
    repo_files: &[RepoFile],
    artifact_files: Vec<ArtifactFile>,
    total_size: u64,
    snapshot_dir: &Path,
    lock: &mut LockFile,
) {
    let candidates_for_format: Vec<CandidateFile> = candidate_files(repo_files);
    let format = match detect_format(
        &candidates_for_format,
        None,
        metadata.library_name.as_deref(),
    ) {
        crate::artifacts::FormatDetection::Detected(fmt) => fmt,
        _ => FormatKind::Unknown,
    };

    let alias = default_alias(&candidate.repo_id, format, None);
    let signature = crate::artifacts::artifact_signature(
        &candidate.repo_id,
        &metadata.commit,
        &format,
        &candidates_for_format,
    );
    let id = format!("hf:{signature}");

    let entry = ModelEntry {
        name: alias,
        source: Source {
            kind: "hf".to_string(),
            repo: candidate.repo_id.clone(),
            revision: "main".to_string(),
            commit: metadata.commit.clone(),
            ownership: Ownership::Managed,
        },
        format,
        artifact: Artifact {
            signature,
            size_bytes: total_size,
            full_snapshot: false,
            files: artifact_files,
        },
        locations: Locations {
            hf_cache: Some(HfCacheLocation {
                snapshot_path: snapshot_dir.to_path_buf(),
            }),
        },
        exposures: BTreeMap::from([(
            candidate.tool.clone(),
            ExposureEntry {
                status: ExposureStatus::Ok,
                strategy: "consolidated-symlink".to_string(),
                path: Some(candidate.local_path.clone()),
                created_by: Some("lmm-consolidate".to_string()),
            },
        )]),
    };

    lock.models.insert(id, entry);
}

// ── File listing ───────────────────────────────────────────────────────

fn list_local_relative_paths(dir: &Path) -> Vec<String> {
    let mut paths = Vec::new();
    collect_relative_paths(dir, dir, &mut paths);
    paths
}

fn collect_relative_paths(base: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = path.symlink_metadata() else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.file_type().is_dir() {
            collect_relative_paths(base, &path, out);
        } else if meta.file_type().is_file()
            && let Ok(rel) = path.strip_prefix(base)
        {
            out.push(rel.to_string_lossy().into_owned());
        }
    }
}

// ── Hash verification ──────────────────────────────────────────────────

fn verify_file_hash(
    local_path: &Path,
    api_file: &RepoFile,
) -> std::result::Result<(), ConsolidateError> {
    let meta = fs::metadata(local_path).map_err(|source| {
        ConsolidateError::App(AppError::Read {
            path: local_path.to_path_buf(),
            source,
        })
    })?;

    if meta.len() != api_file.size_bytes {
        return Err(ConsolidateError::Skip(SkipReason::HashMismatch(
            api_file.path.clone(),
        )));
    }

    let is_lfs = api_file.oid.len() == 64;
    let computed = if is_lfs {
        compute_sha256(local_path)?
    } else {
        compute_git_sha1(local_path, meta.len())?
    };

    if computed != api_file.oid {
        return Err(ConsolidateError::Skip(SkipReason::HashMismatch(
            api_file.path.clone(),
        )));
    }
    Ok(())
}

fn compute_sha256(path: &Path) -> std::result::Result<String, ConsolidateError> {
    let mut hasher = sha2::Sha256::new();
    feed_file(path, |chunk| sha1::Digest::update(&mut hasher, chunk))?;
    Ok(format!("{:x}", sha1::Digest::finalize(hasher)))
}

fn compute_git_sha1(path: &Path, size: u64) -> std::result::Result<String, ConsolidateError> {
    let mut hasher = sha1::Sha1::new();
    sha1::Digest::update(&mut hasher, format!("blob {size}\0").as_bytes());
    feed_file(path, |chunk| sha1::Digest::update(&mut hasher, chunk))?;
    Ok(format!("{:x}", sha1::Digest::finalize(hasher)))
}

fn feed_file(
    path: &Path,
    mut sink: impl FnMut(&[u8]),
) -> std::result::Result<(), ConsolidateError> {
    let mut file = fs::File::open(path).map_err(|source| {
        ConsolidateError::App(AppError::Read {
            path: path.to_path_buf(),
            source,
        })
    })?;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).map_err(|source| {
            ConsolidateError::App(AppError::Read {
                path: path.to_path_buf(),
                source,
            })
        })?;
        if n == 0 {
            return Ok(());
        }
        sink(&buf[..n]);
    }
}

// ── Validation ────────────────────────────────────────────────────────

fn validate_oid(oid: &str) -> std::result::Result<(), ConsolidateError> {
    if oid.is_empty()
        || oid.contains('/')
        || oid.contains("..")
        || !oid.chars().all(|c| c.is_ascii_hexdigit())
    {
        return Err(ConsolidateError::App(AppError::InvalidInput(format!(
            "invalid blob oid: {oid}"
        ))));
    }
    Ok(())
}

// ── Helpers ────────────────────────────────────────────────────────────

fn create_dir(path: &Path) -> std::result::Result<(), ConsolidateError> {
    fs::create_dir_all(path).map_err(|source| {
        ConsolidateError::App(AppError::CreateDir {
            path: path.to_path_buf(),
            source,
        })
    })?;
    Ok(())
}
