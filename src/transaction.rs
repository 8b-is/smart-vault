use std::collections::BTreeMap;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{ErrorKind, Read, Write};
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::Utc;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{MemVaultError, Result};
use crate::paths::{safe_vault_join, to_posix_rel};

pub const TRANSACTION_SCHEMA: &str = "smart-vault.transaction.v1";
/// Maximum single-file read size (64 MiB) to prevent OOM on oversized files.
pub const MAX_SINGLE_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// Supported atomic filesystem operations within a vault transaction.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum FileOp {
    Mkdir {
        path: String,
    },
    Create {
        path: String,
        content: String,
    },
    Update {
        path: String,
        content: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_sha256: Option<String>,
    },
    Delete {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_sha256: Option<String>,
    },
}

impl FileOp {
    #[must_use]
    pub fn path(&self) -> &str {
        match self {
            Self::Mkdir { path }
            | Self::Create { path, .. }
            | Self::Update { path, .. }
            | Self::Delete { path, .. } => path,
        }
    }
}

/// Declarative transaction bundle reviewed before applying mutations to the vault.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionBundle {
    #[serde(default = "default_schema")]
    pub schema: String,
    pub operation_id: String,
    #[serde(default = "default_op_type")]
    pub operation_type: String,
    #[serde(default)]
    pub generated_at: String,
    #[serde(default)]
    pub description: String,
    pub operations: Vec<FileOp>,
}

fn default_schema() -> String {
    TRANSACTION_SCHEMA.to_string()
}

fn default_op_type() -> String {
    "generic".to_string()
}

/// Deterministic inspection result emitted during dry-run review.
#[derive(Debug, Clone, Serialize)]
pub struct TransactionPlan {
    pub schema: String,
    pub operation_id: String,
    pub operation_type: String,
    pub vault_root: String,
    pub operation_count: usize,
    pub approval_sha256: String,
    pub approval_hint: String,
    pub operations: Vec<FileOp>,
}

/// Pre-image entry saved inside a transaction journal for rollback.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreImage {
    pub path: String,
    pub existed: bool,
    pub content_base64_or_utf8: Option<String>,
    pub sha256: Option<String>,
}

/// Crash-safe journal persisted under `.vault-meta/journal/<operation_id>.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionJournal {
    pub operation_id: String,
    pub operation_type: String,
    pub approval_sha256: String,
    pub status: String, // "in_progress" | "committed" | "recovered"
    pub started_at: String,
    pub completed_at: Option<String>,
    pub pre_images: Vec<PreImage>,
}

/// Exclusive advisory vault lock over `.vault-meta/mutation.lock`.
pub struct MutationLock {
    file: File,
}

impl MutationLock {
    pub fn acquire(vault_root: &Path) -> Result<Self> {
        let meta_dir = safe_vault_join(vault_root, Path::new(".vault-meta"))?;
        fs::create_dir_all(&meta_dir)?;
        let lock_path = safe_vault_join(vault_root, Path::new(".vault-meta/mutation.lock"))?;
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)?;
        file.try_lock_exclusive()
            .map_err(|err| MemVaultError::LockBusy(format!("{}: {err}", lock_path.display())))?;
        Ok(Self { file })
    }
}

impl Drop for MutationLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

/// Computes lowercase hex SHA-256 of arbitrary bytes.
#[must_use]
pub fn sha256_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// Constant-time string comparison for SHA-256 digests (delegated to `#![no_std]` asm barrier).
#[inline]
#[must_use]
pub fn constant_time_eq(a: &str, b: &str) -> bool {
    smart_vault_core::asm::ct_eq_bytes(a.as_bytes(), b.as_bytes())
}

/// Ensures `path` (if it exists) is not a symlink.
pub fn assert_not_symlink(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Err(MemVaultError::SymlinkEscape(format!(
            "refusing to access symlink {}",
            path.display()
        ))),
        Ok(_) => Ok(()),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.into()),
    }
}

fn inspect_writable_target(target: &Path) -> Result<Option<Metadata>> {
    match fs::symlink_metadata(target) {
        Ok(meta) if meta.file_type().is_symlink() => Err(MemVaultError::SymlinkEscape(format!(
            "refusing to write to symlink {}",
            target.display()
        ))),
        Ok(meta) if meta.is_dir() => Err(MemVaultError::InvalidPath(format!(
            "cannot overwrite directory {}",
            target.display()
        ))),
        Ok(meta) => Ok(Some(meta)),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
    }
}

/// Reads at most `max_bytes` from a non-symlink regular file, failing closed on symlinks or oversized files.
pub fn read_bounded_bytes(path: &Path, max_bytes: u64) -> Result<Vec<u8>> {
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        return Err(MemVaultError::SymlinkEscape(format!(
            "refusing to read symlink {}",
            path.display()
        )));
    }
    if !meta.is_file() {
        return Err(MemVaultError::InvalidPath(format!(
            "{} is not a regular file",
            path.display()
        )));
    }
    if meta.len() > max_bytes {
        return Err(MemVaultError::Validation(format!(
            "{} exceeds maximum read limit of {} bytes",
            path.display(),
            max_bytes
        )));
    }

    let file = File::open(path)?;
    let fd_meta = file.metadata()?;
    if fd_meta.len() > max_bytes {
        return Err(MemVaultError::Validation(format!(
            "{} exceeds maximum read limit of {} bytes",
            path.display(),
            max_bytes
        )));
    }

    let cap = usize::try_from(fd_meta.len().min(max_bytes)).unwrap_or(0);
    let mut buf = Vec::with_capacity(cap);
    file.take(max_bytes.saturating_add(1)).read_to_end(&mut buf)?;
    if buf.len() as u64 > max_bytes {
        return Err(MemVaultError::Validation(format!(
            "{} exceeds maximum read limit of {} bytes",
            path.display(),
            max_bytes
        )));
    }
    Ok(buf)
}

/// Reads a bounded UTF-8 file, failing closed on symlinks or oversized files.
pub fn read_bounded_string(path: &Path, max_bytes: u64) -> Result<String> {
    let bytes = read_bounded_bytes(path, max_bytes)?;
    String::from_utf8(bytes)
        .map_err(|err| MemVaultError::Io(std::io::Error::new(ErrorKind::InvalidData, err)))
}

/// Atomically writes `bytes` to `target` using a sibling temp file + rename, rejecting symlinks.
pub fn atomic_write_bytes(target: &Path, bytes: &[u8]) -> Result<()> {
    atomic_write_bytes_with_mode(target, bytes, None)
}

/// Atomically writes `bytes` to `target` with an optional Unix file mode applied before rename.
pub fn atomic_write_bytes_with_mode(target: &Path, bytes: &[u8], mode: Option<u32>) -> Result<()> {
    let file_name = target
        .file_name()
        .ok_or_else(|| {
            MemVaultError::InvalidPath(format!("missing file name for {}", target.display()))
        })?
        .to_string_lossy()
        .into_owned();
    let parent = target.parent().ok_or_else(|| {
        MemVaultError::InvalidPath(format!("missing parent for {}", target.display()))
    })?;

    let existing_meta = inspect_writable_target(target)?;
    #[cfg(unix)]
    let effective_mode: Option<u32> =
        mode.or_else(|| existing_meta.as_ref().map(|m| m.permissions().mode() & 0o777));
    #[cfg(not(unix))]
    let _ = (&existing_meta, mode);

    fs::create_dir_all(parent)?;
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let tmp_path = parent.join(format!(".{file_name}.{}.{}.tmp", std::process::id(), nanos));

    let write_res: Result<()> = (|| {
        let mut opts = OpenOptions::new();
        opts.create_new(true).write(true);
        #[cfg(unix)]
        if let Some(m) = effective_mode {
            opts.mode(m);
        }
        let mut f = opts.open(&tmp_path)?;
        #[cfg(unix)]
        if let Some(m) = effective_mode {
            f.set_permissions(fs::Permissions::from_mode(m))?;
        }
        f.write_all(bytes)?;
        f.sync_all()?;
        Ok(())
    })();

    if let Err(err) = write_res {
        let _ = fs::remove_file(&tmp_path);
        return Err(err);
    }

    if let Err(err) = inspect_writable_target(target) {
        let _ = fs::remove_file(&tmp_path);
        return Err(err);
    }

    if let Err(err) = fs::rename(&tmp_path, target) {
        let _ = fs::remove_file(&tmp_path);
        return Err(err.into());
    }
    Ok(())
}

/// Validates every operation path and computes the deterministic approval SHA-256 hash.
pub fn inspect_bundle(vault_root: &Path, bundle: &TransactionBundle) -> Result<TransactionPlan> {
    if bundle.operation_id.trim().is_empty() {
        return Err(MemVaultError::Validation("operation_id must be non-empty".to_string()));
    }

    let canonical_root = fs::canonicalize(vault_root)?;
    let mut normalized_ops = Vec::with_capacity(bundle.operations.len());
    for op in &bundle.operations {
        let target = safe_vault_join(&canonical_root, Path::new(op.path()))?;
        let rel = to_posix_rel(&canonical_root, &target);
        let norm = match op {
            FileOp::Mkdir { .. } => FileOp::Mkdir { path: rel },
            FileOp::Create { content, .. } => {
                FileOp::Create { path: rel, content: content.clone() }
            }
            FileOp::Update { content, expected_sha256, .. } => FileOp::Update {
                path: rel,
                content: content.clone(),
                expected_sha256: expected_sha256.clone(),
            },
            FileOp::Delete { expected_sha256, .. } => {
                FileOp::Delete { path: rel, expected_sha256: expected_sha256.clone() }
            }
        };
        normalized_ops.push(norm);
    }

    normalized_ops.sort_by(|a, b| a.path().cmp(b.path()));

    let canonical_payload = serde_json::json!({
        "schema": TRANSACTION_SCHEMA,
        "operation_id": bundle.operation_id,
        "operation_type": bundle.operation_type,
        "generated_at": bundle.generated_at,
        "operations": normalized_ops,
    });
    let canonical_bytes = serde_json::to_vec(&canonical_payload)?;
    let approval_sha256 = sha256_bytes(&canonical_bytes);

    Ok(TransactionPlan {
        schema: TRANSACTION_SCHEMA.to_string(),
        operation_id: bundle.operation_id.clone(),
        operation_type: bundle.operation_type.clone(),
        vault_root: vault_root.display().to_string(),
        operation_count: normalized_ops.len(),
        approval_hint: format!(
            "review the plan, then run with --approved-plan-sha256 {approval_sha256} --apply"
        ),
        approval_sha256,
        operations: normalized_ops,
    })
}

/// Applies an approved transaction bundle under lock with pre-image journaling.
pub fn apply_bundle(
    vault_root: &Path,
    bundle: &TransactionBundle,
    approved_sha256: Option<&str>,
) -> Result<TransactionJournal> {
    let supplied = approved_sha256
        .filter(|s| !s.trim().is_empty())
        .ok_or(MemVaultError::PlanApprovalRequired)?;

    let plan = inspect_bundle(vault_root, bundle)?;
    if !constant_time_eq(supplied.trim(), &plan.approval_sha256) {
        return Err(MemVaultError::PlanChanged);
    }

    let _lock = MutationLock::acquire(vault_root)?;

    // Validate preconditions & capture pre-images before touching any file
    let mut pre_images_map: BTreeMap<String, PreImage> = BTreeMap::new();
    for op in &plan.operations {
        let abs = safe_vault_join(vault_root, Path::new(op.path()))?;
        let target_meta = match fs::symlink_metadata(&abs) {
            Ok(meta) => {
                if meta.file_type().is_symlink() {
                    return Err(MemVaultError::SymlinkEscape(format!(
                        "refusing to mutate symlink {}",
                        abs.display()
                    )));
                }
                Some(meta)
            }
            Err(err) if err.kind() == ErrorKind::NotFound => None,
            Err(err) => return Err(err.into()),
        };

        let mut cached_content: Option<(String, String)> = None;
        match op {
            FileOp::Create { .. } => {
                if target_meta.is_some() {
                    return Err(MemVaultError::Transaction(format!(
                        "Create target already exists: {}",
                        op.path()
                    )));
                }
            }
            FileOp::Update { expected_sha256, .. } | FileOp::Delete { expected_sha256, .. } => {
                let Some(meta) = target_meta.as_ref() else {
                    return Err(MemVaultError::Transaction(format!(
                        "Update/Delete target does not exist: {}",
                        op.path()
                    )));
                };
                if !meta.is_file() {
                    return Err(MemVaultError::Transaction(format!(
                        "Update/Delete target is not a regular file: {}",
                        op.path()
                    )));
                }
                let raw = read_bounded_string(&abs, MAX_SINGLE_FILE_BYTES)?;
                let current = sha256_bytes(raw.as_bytes());
                if let Some(expected) = expected_sha256 {
                    if !constant_time_eq(&current, expected) {
                        return Err(MemVaultError::Transaction(format!(
                            "sha256 mismatch for {}: expected {expected}, found {current}",
                            op.path()
                        )));
                    }
                }
                cached_content = Some((raw, current));
            }
            FileOp::Mkdir { .. } => {}
        }

        if !pre_images_map.contains_key(op.path()) {
            if let Some((raw, hash)) = cached_content {
                pre_images_map.insert(
                    op.path().to_string(),
                    PreImage {
                        path: op.path().to_string(),
                        existed: true,
                        content_base64_or_utf8: Some(raw),
                        sha256: Some(hash),
                    },
                );
            } else if target_meta.as_ref().is_some_and(Metadata::is_file) {
                let raw = read_bounded_string(&abs, MAX_SINGLE_FILE_BYTES)?;
                let hash = sha256_bytes(raw.as_bytes());
                pre_images_map.insert(
                    op.path().to_string(),
                    PreImage {
                        path: op.path().to_string(),
                        existed: true,
                        content_base64_or_utf8: Some(raw),
                        sha256: Some(hash),
                    },
                );
            } else {
                pre_images_map.insert(
                    op.path().to_string(),
                    PreImage {
                        path: op.path().to_string(),
                        existed: target_meta.is_some(),
                        content_base64_or_utf8: None,
                        sha256: None,
                    },
                );
            }
        }
    }

    let journal_dir = safe_vault_join(vault_root, Path::new(".vault-meta/journal"))?;
    fs::create_dir_all(&journal_dir)?;
    let journal_path = journal_dir.join(format!("{}.json", sanitize_op_id(&plan.operation_id)));

    let mut journal = TransactionJournal {
        operation_id: plan.operation_id.clone(),
        operation_type: plan.operation_type.clone(),
        approval_sha256: plan.approval_sha256.clone(),
        status: "in_progress".to_string(),
        started_at: Utc::now().to_rfc3339(),
        completed_at: None,
        pre_images: pre_images_map.into_values().collect(),
    };
    atomic_write_bytes(&journal_path, &serde_json::to_vec_pretty(&journal)?)?;

    // Apply operations in deterministic order
    for op in &plan.operations {
        let abs = safe_vault_join(vault_root, Path::new(op.path()))?;
        assert_not_symlink(&abs)?;
        match op {
            FileOp::Mkdir { .. } => {
                fs::create_dir_all(&abs)?;
            }
            FileOp::Create { content, .. } | FileOp::Update { content, .. } => {
                atomic_write_bytes(&abs, content.as_bytes())?;
            }
            FileOp::Delete { .. } => match fs::symlink_metadata(&abs) {
                Ok(meta) if meta.file_type().is_symlink() => {
                    return Err(MemVaultError::SymlinkEscape(format!(
                        "refusing to delete symlink {}",
                        abs.display()
                    )));
                }
                Ok(meta) if meta.is_file() => fs::remove_file(&abs)?,
                Ok(_) => {}
                Err(err) if err.kind() == ErrorKind::NotFound => {}
                Err(err) => return Err(err.into()),
            },
        }
    }

    journal.status = "committed".to_string();
    journal.completed_at = Some(Utc::now().to_rfc3339());
    atomic_write_bytes(&journal_path, &serde_json::to_vec_pretty(&journal)?)?;
    Ok(journal)
}

/// Summary returned by transaction recovery.
#[derive(Debug, Clone, Serialize)]
pub struct RecoveryReport {
    pub dry_run: bool,
    pub incomplete_count: usize,
    pub recovered_ids: Vec<String>,
    pub restored_paths: Vec<String>,
}

/// Inspects `.vault-meta/journal/*.json` for `in_progress` transactions and rolls them back if `apply == true`.
pub fn recover_incomplete(vault_root: &Path, apply: bool) -> Result<RecoveryReport> {
    let journal_dir = safe_vault_join(vault_root, Path::new(".vault-meta/journal"))?;
    if !journal_dir.is_dir() {
        return Ok(RecoveryReport {
            dry_run: !apply,
            incomplete_count: 0,
            recovered_ids: Vec::new(),
            restored_paths: Vec::new(),
        });
    }

    let _lock = if apply { Some(MutationLock::acquire(vault_root)?) } else { None };

    let mut journal_paths: Vec<PathBuf> = fs::read_dir(&journal_dir)?
        .filter_map(|e| e.ok().map(|entry| entry.path()))
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
        .collect();
    journal_paths.sort();

    let mut recovered_ids = Vec::new();
    let mut restored_paths = Vec::new();

    for jpath in journal_paths {
        let raw = read_bounded_string(&jpath, MAX_SINGLE_FILE_BYTES)?;
        let mut journal: TransactionJournal = serde_json::from_str(&raw)?;
        if journal.status != "in_progress" {
            continue;
        }
        recovered_ids.push(journal.operation_id.clone());

        for pre in &journal.pre_images {
            restored_paths.push(pre.path.clone());
            if apply {
                let abs = safe_vault_join(vault_root, Path::new(&pre.path))?;
                if pre.existed {
                    if let Some(ref content) = pre.content_base64_or_utf8 {
                        atomic_write_bytes(&abs, content.as_bytes())?;
                    }
                } else {
                    match fs::symlink_metadata(&abs) {
                        Ok(meta) if meta.file_type().is_symlink() => {
                            return Err(MemVaultError::SymlinkEscape(format!(
                                "refusing to delete symlink {}",
                                abs.display()
                            )));
                        }
                        Ok(meta) if meta.is_file() => fs::remove_file(&abs)?,
                        Ok(_) => {}
                        Err(err) if err.kind() == ErrorKind::NotFound => {}
                        Err(err) => return Err(err.into()),
                    }
                }
            }
        }

        if apply {
            journal.status = "recovered".to_string();
            journal.completed_at = Some(Utc::now().to_rfc3339());
            atomic_write_bytes(&jpath, &serde_json::to_vec_pretty(&journal)?)?;
        }
    }

    Ok(RecoveryReport {
        dry_run: !apply,
        incomplete_count: recovered_ids.len(),
        recovered_ids,
        restored_paths,
    })
}

/// Counts incomplete (`in_progress`) transaction journals without mutating the vault.
#[must_use]
pub fn count_incomplete_journals(vault_root: &Path) -> usize {
    let Ok(journal_dir) = safe_vault_join(vault_root, Path::new(".vault-meta/journal")) else {
        return 0;
    };
    let Ok(entries) = fs::read_dir(journal_dir) else {
        return 0;
    };
    entries
        .filter_map(|e| e.ok().map(|entry| entry.path()))
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
        .filter(|p| {
            read_bounded_string(p, MAX_SINGLE_FILE_BYTES)
                .ok()
                .and_then(|raw| serde_json::from_str::<TransactionJournal>(&raw).ok())
                .is_some_and(|j| j.status == "in_progress")
        })
        .count()
}

fn sanitize_op_id(id: &str) -> String {
    id.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect()
}
