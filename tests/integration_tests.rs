//! Integration tests for lmm CLI operations.
//! These tests use tempdir for isolated state and verify lock.json,
//! filesystem, and adapter behavior without touching real tool directories.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

// ── Lock file operations (tempdir-dependent) ───────────────────────────

#[test]
fn lock_file_roundtrip_preserves_all_fields() {
    let dir = tempfile::tempdir().unwrap();
    let lock_path = dir.path().join("lock.json");

    let mut lock = lmm::lock::LockFile::default();
    lock.models.insert(
        "test-id".to_string(),
        lmm::model::ModelEntry {
            name: "test-model".to_string(),
            source: lmm::model::Source {
                kind: "hf".to_string(),
                repo: "org/repo".to_string(),
                revision: "main".to_string(),
                commit: "abc123".to_string(),
                ownership: lmm::model::Ownership::Managed,
            },
            format: lmm::model::FormatKind::Mlx,
            artifact: lmm::model::Artifact {
                signature: "sig".to_string(),
                size_bytes: 1000,
                full_snapshot: false,
                files: vec![lmm::model::ArtifactFile {
                    path: "model.safetensors".to_string(),
                    size_bytes: 900,
                    hf_blob: Some("blobhash".to_string()),
                    sha256: None,
                    role: None,
                }],
            },
            locations: lmm::model::Locations {
                hf_cache: Some(lmm::model::HfCacheLocation {
                    snapshot_path: dir.path().join("snapshot"),
                }),
            },
            exposures: BTreeMap::new(),
        },
    );

    lock.save(&lock_path).unwrap();
    let loaded = lmm::lock::LockFile::load(&lock_path).unwrap();

    assert_eq!(loaded.models.len(), 1);
    let model = loaded.models.get("test-id").unwrap();
    assert_eq!(model.name, "test-model");
    assert_eq!(model.format, lmm::model::FormatKind::Mlx);
    assert_eq!(model.source.repo, "org/repo");
    assert_eq!(model.source.ownership, lmm::model::Ownership::Managed);
    assert_eq!(model.artifact.size_bytes, 1000);
    assert_eq!(model.artifact.files.len(), 1);
    assert_eq!(
        model.artifact.files[0].hf_blob,
        Some("blobhash".to_string())
    );
}

#[test]
fn lock_file_corrupt_json_returns_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lock.json");
    fs::write(&path, b"not json").unwrap();
    assert!(lmm::lock::LockFile::load(&path).is_err());
}

// ── Symlink exposure (filesystem) ───────────────────────────────────────

#[cfg(unix)]
#[test]
fn expose_symlink_tree_creates_and_resolves() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tool-root");
    let snapshot = dir.path().join("snapshot");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&snapshot).unwrap();
    fs::write(snapshot.join("model.safetensors"), b"data").unwrap();
    fs::write(snapshot.join("config.json"), b"{}").unwrap();

    let files = vec![
        lmm::hf::RepoFile {
            path: "model.safetensors".to_string(),
            size_bytes: 4,
            oid: "o1".to_string(),
        },
        lmm::hf::RepoFile {
            path: "config.json".to_string(),
            size_bytes: 2,
            oid: "o2".to_string(),
        },
    ];

    let target = root.join("test-model");
    let entry =
        lmm::adapters::expose_symlink_tree(&root, &target, &snapshot, &files, false, "symlink")
            .unwrap();

    assert_eq!(entry.status, lmm::model::ExposureStatus::Ok);
    assert!(target.join("model.safetensors").exists());
    assert!(target.join("config.json").exists());
    assert!(
        target
            .join("model.safetensors")
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[cfg(unix)]
#[test]
fn expose_symlink_tree_refuses_existing_without_replace() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tool-root");
    let snapshot = dir.path().join("snapshot");
    let target = root.join("existing");
    fs::create_dir_all(&target).unwrap();
    fs::create_dir_all(&snapshot).unwrap();

    let files = vec![];
    let result =
        lmm::adapters::expose_symlink_tree(&root, &target, &snapshot, &files, false, "symlink");
    assert!(result.is_err());
}

#[cfg(unix)]
#[test]
fn expose_symlink_tree_replaces_with_flag() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("tool-root");
    let snapshot = dir.path().join("snapshot");
    let target = root.join("existing");
    fs::create_dir_all(&target).unwrap();
    fs::create_dir_all(&snapshot).unwrap();
    fs::write(snapshot.join("f.txt"), b"x").unwrap();

    let files = vec![lmm::hf::RepoFile {
        path: "f.txt".to_string(),
        size_bytes: 1,
        oid: "o".to_string(),
    }];
    let entry =
        lmm::adapters::expose_symlink_tree(&root, &target, &snapshot, &files, true, "symlink")
            .unwrap();
    assert_eq!(entry.status, lmm::model::ExposureStatus::Ok);
    assert!(target.join("f.txt").exists());
}

#[cfg(unix)]
#[test]
fn remove_directory_exposure_deletes_dir() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("model");
    fs::create_dir_all(&target).unwrap();
    fs::write(target.join("file.txt"), b"data").unwrap();

    let entry = lmm::model::ExposureEntry {
        status: lmm::model::ExposureStatus::Ok,
        strategy: "symlink".to_string(),
        path: Some(target.clone()),
        created_by: Some("lmm".to_string()),
    };
    lmm::adapters::remove_directory_exposure(&entry, dir.path()).unwrap();
    assert!(!target.exists());
}

#[test]
fn remove_directory_exposure_nonexistent_is_ok() {
    let entry = lmm::model::ExposureEntry {
        status: lmm::model::ExposureStatus::Ok,
        strategy: "symlink".to_string(),
        path: Some(std::path::PathBuf::from("/nonexistent/path")),
        created_by: Some("lmm".to_string()),
    };
    assert!(
        lmm::adapters::remove_directory_exposure(&entry, std::path::Path::new("/nonexistent"))
            .is_ok()
    );
}

#[test]
fn remove_directory_exposure_rejects_outside_root() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("root");
    let outside = dir.path().join("outside");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&outside).unwrap();

    let entry = lmm::model::ExposureEntry {
        status: lmm::model::ExposureStatus::Ok,
        strategy: "symlink".to_string(),
        path: Some(outside.clone()),
        created_by: Some("lmm".to_string()),
    };
    assert!(lmm::adapters::remove_directory_exposure(&entry, &root).is_err());
    assert!(outside.exists());
}

// ── CLI binary behavior (isolated process state) ───────────────────────

#[test]
fn cli_help_prints_usage() {
    let env = CliEnv::new();

    let output = env.run(["--help"]);

    assert_success(&output);
    assert!(stdout(&output).contains("Local AI model manager"));
}

#[test]
fn cli_unknown_subcommand_exits_with_usage_error() {
    let env = CliEnv::new();

    let output = env.run(["definitely-not-a-command"]);

    assert!(!output.status.success());
    assert!(stderr(&output).contains("unrecognized subcommand"));
}

#[test]
fn cli_config_path_uses_lmm_state_dir() {
    let env = CliEnv::new();

    let output = env.run(["config", "path"]);

    assert_success(&output);
    assert_eq!(
        stdout(&output).trim(),
        env.state_dir.join("config.json").display().to_string()
    );
}

#[test]
fn cli_config_set_then_get_roundtrips_value() {
    let env = CliEnv::new();

    assert_success(&env.run(["config", "set", "defaults.format", "gguf"]));
    let output = env.run(["config", "get", "defaults.format"]);

    assert_success(&output);
    assert_eq!(stdout(&output).trim(), "gguf");
}

#[test]
fn cli_config_set_rejects_unknown_adapter_default() {
    let env = CliEnv::new();

    let output = env.run(["config", "set", "defaults.default_tools", "missing-tool"]);

    assert!(!output.status.success());
    assert!(stderr(&output).contains("unknown adapter `missing-tool`"));
}

#[test]
fn cli_completions_zsh_outputs_completion_script() {
    let env = CliEnv::new();

    let output = env.run(["completions", "zsh"]);

    assert_success(&output);
    assert!(stdout(&output).contains("#compdef lmm"));
}

#[test]
fn cli_list_json_outputs_empty_array_for_empty_state() {
    let env = CliEnv::new();

    let output = env.run(["list", "--json"]);

    assert_success(&output);
    assert_eq!(stdout(&output).trim(), "[]");
}

#[test]
fn cli_list_json_reads_tracked_model_from_lock() {
    let env = CliEnv::new();
    write_lock_with_fixture_model(&env, "test-model");

    let output = env.run(["list", "--json"]);

    assert_success(&output);
    let rows: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(rows[0]["name"], "test-model");
}

#[test]
fn cli_info_json_reads_tracked_model_from_lock() {
    let env = CliEnv::new();
    write_lock_with_fixture_model(&env, "test-model");

    let output = env.run(["info", "test-model", "--json"]);

    assert_success(&output);
    let model: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(model["source"]["repo"], "org/repo");
}

#[test]
fn cli_remove_without_target_fails_in_non_interactive_process() {
    let env = CliEnv::new();

    let output = env.run(["remove"]);

    assert!(!output.status.success());
    assert!(stderr(&output).contains("remove needs a model name or --all"));
}

#[test]
fn cli_add_without_repo_shows_usage_and_exits_ok() {
    let env = CliEnv::new();

    let output = env.run(["add"]);

    assert!(!output.status.success());
    assert!(stderr(&output).contains("Missing required argument"));
    assert!(stderr(&output).contains("lmm add"));
}

#[test]
fn cli_add_unknown_tool_fails_before_network_access() {
    let env = CliEnv::new();

    let output = env.run(["add", "org/repo", "--tool", "missing-tool", "--yes"]);

    assert!(!output.status.success());
    assert!(stderr(&output).contains("unknown adapter `missing-tool`"));
}

#[test]
fn cli_config_set_rejects_invalid_format() {
    let env = CliEnv::new();

    let output = env.run(["config", "set", "defaults.format", "onnx"]);

    assert!(!output.status.success());
    assert!(stderr(&output).contains("unknown format: onnx"));
}

#[test]
fn cli_list_json_filters_by_format() {
    let env = CliEnv::new();
    write_lock_with_fixture_model(&env, "test-model");

    let output = env.run(["list", "--json", "--format", "mlx"]);

    assert_success(&output);
    assert_eq!(stdout(&output).trim(), "[]");
}

#[test]
fn cli_list_json_filters_by_exposure_tool() {
    let env = CliEnv::new();
    write_lock_with_fixture_model_and_exposure(&env, "test-model", "lmstudio");

    let output = env.run(["list", "--json", "--tool", "lmstudio"]);

    assert_success(&output);
    let rows: serde_json::Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(rows[0]["where"], "lmstudio");
}

#[test]
fn cli_info_missing_model_exits_successfully_with_message() {
    let env = CliEnv::new();

    let output = env.run(["info", "missing-model"]);

    assert!(!output.status.success());
    assert!(stderr(&output).contains("model not found: missing-model"));
}

#[test]
fn cli_remove_dry_run_keeps_lock_entry_and_snapshot_file() {
    let env = CliEnv::new();
    let snapshot = write_lock_with_fixture_model(&env, "test-model");

    let output = env.run(["remove", "test-model", "--dry-run"]);

    assert_success(&output);
    assert!(snapshot.join("model.gguf").exists());
    let lock = lmm::lock::LockFile::load(&env.state_dir.join("lock.json")).unwrap();
    assert!(lock.models.contains_key("fixture-id"));
}

#[test]
fn cli_remove_all_yes_removes_lock_entry_and_snapshot_file() {
    let env = CliEnv::new();
    let snapshot = write_lock_with_fixture_model(&env, "test-model");

    let output = env.run(["remove", "--all", "--yes"]);

    assert_success(&output);
    assert!(!snapshot.join("model.gguf").exists());
    let lock = lmm::lock::LockFile::load(&env.state_dir.join("lock.json")).unwrap();
    assert!(lock.models.is_empty());
}

#[test]
fn cli_doctor_reports_missing_snapshot_file() {
    let env = CliEnv::new();
    let snapshot = write_lock_with_fixture_model(&env, "test-model");
    fs::remove_file(snapshot.join("model.gguf")).unwrap();

    let output = env.run(["doctor"]);

    assert_success(&output);
    assert!(stderr(&output).contains("missing snapshot files"));
    assert!(stderr(&output).contains("1"));
}

#[test]
fn cli_doctor_fix_marks_stale_exposure_in_lock() {
    let env = CliEnv::new();
    write_lock_with_fixture_model_and_exposure(&env, "test-model", "lmstudio");

    let output = env.run(["doctor", "--fix"]);

    assert_success(&output);
    let lock = lmm::lock::LockFile::load(&env.state_dir.join("lock.json")).unwrap();
    let status = &lock.models["fixture-id"].exposures["lmstudio"].status;
    assert_eq!(*status, lmm::model::ExposureStatus::Stale);
}

#[test]
fn cli_gc_dry_run_reports_state_tmp_file_without_removing_it() {
    let env = CliEnv::new();
    let tmp_file = env.state_dir.join("tmp/downloads/file.tmp");
    fs::create_dir_all(tmp_file.parent().unwrap()).unwrap();
    fs::write(&tmp_file, b"tmp").unwrap();

    let output = env.run(["gc"]);

    assert_success(&output);
    assert!(stderr(&output).contains("removable files"));
    assert!(tmp_file.exists());
}

#[test]
fn cli_gc_yes_removes_state_tmp_file() {
    let env = CliEnv::new();
    let tmp_file = env.state_dir.join("tmp/downloads/file.tmp");
    fs::create_dir_all(tmp_file.parent().unwrap()).unwrap();
    fs::write(&tmp_file, b"tmp").unwrap();

    let output = env.run(["gc", "--yes"]);

    assert_success(&output);
    assert!(!tmp_file.exists());
}

struct CliEnv {
    _dir: tempfile::TempDir,
    state_dir: PathBuf,
    home_dir: PathBuf,
    hf_cache_dir: PathBuf,
}

impl CliEnv {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path().join("state");
        let home_dir = dir.path().join("home");
        let hf_cache_dir = dir.path().join("hf-cache");
        fs::create_dir_all(&state_dir).unwrap();
        fs::create_dir_all(&home_dir).unwrap();
        fs::create_dir_all(&hf_cache_dir).unwrap();
        Self {
            _dir: dir,
            state_dir,
            home_dir,
            hf_cache_dir,
        }
    }

    fn run<const N: usize>(&self, args: [&str; N]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_lmm"))
            .args(args)
            .env("LMM_STATE_DIR", &self.state_dir)
            .env("HOME", &self.home_dir)
            .env("HUGGINGFACE_HUB_CACHE", &self.hf_cache_dir)
            .env("NO_COLOR", "1")
            .output()
            .unwrap()
    }
}

fn write_lock_with_fixture_model(env: &CliEnv, name: &str) -> PathBuf {
    let snapshot = env.state_dir.join("snapshot");
    fs::create_dir_all(&snapshot).unwrap();
    fs::write(snapshot.join("model.gguf"), b"model").unwrap();

    let mut lock = lmm::lock::LockFile::default();
    lock.models.insert(
        "fixture-id".to_string(),
        fixture_model(name, snapshot.clone()),
    );
    lock.save(&env.state_dir.join("lock.json")).unwrap();
    snapshot
}

fn write_lock_with_fixture_model_and_exposure(env: &CliEnv, name: &str, tool: &str) -> PathBuf {
    let snapshot = env.state_dir.join("snapshot");
    fs::create_dir_all(&snapshot).unwrap();
    fs::write(snapshot.join("model.gguf"), b"model").unwrap();
    let mut model = fixture_model(name, snapshot.clone());
    model.exposures.insert(
        tool.to_string(),
        lmm::model::ExposureEntry {
            status: lmm::model::ExposureStatus::Ok,
            strategy: "symlink".to_string(),
            path: Some(env.state_dir.join("missing-exposure")),
            created_by: Some("lmm".to_string()),
        },
    );

    let mut lock = lmm::lock::LockFile::default();
    lock.models.insert("fixture-id".to_string(), model);
    lock.save(&env.state_dir.join("lock.json")).unwrap();
    snapshot
}

fn fixture_model(name: &str, snapshot: PathBuf) -> lmm::model::ModelEntry {
    lmm::model::ModelEntry {
        name: name.to_string(),
        source: lmm::model::Source {
            kind: "hf".to_string(),
            repo: "org/repo".to_string(),
            revision: "main".to_string(),
            commit: "abcdef123456".to_string(),
            ownership: lmm::model::Ownership::Managed,
        },
        format: lmm::model::FormatKind::Gguf,
        artifact: lmm::model::Artifact {
            signature: "signature".to_string(),
            size_bytes: 5,
            full_snapshot: false,
            files: vec![lmm::model::ArtifactFile {
                path: "model.gguf".to_string(),
                size_bytes: 5,
                hf_blob: Some("blobhash".to_string()),
                sha256: None,
                role: None,
            }],
        },
        locations: lmm::model::Locations {
            hf_cache: Some(lmm::model::HfCacheLocation {
                snapshot_path: snapshot,
            }),
        },
        exposures: BTreeMap::new(),
    }
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "command failed\nstdout:\n{}\nstderr:\n{}",
        stdout(output),
        stderr(output)
    );
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}
