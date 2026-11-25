use std::env;
use std::fs;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

use serde::Serialize;

use crate::error::{MemVaultError, Result};

/// Describes how a vault root was selected.
#[derive(Debug, Clone, Serialize)]
pub struct VaultSelection {
    pub root: PathBuf,
    pub source: String,
    pub initialized: bool,
}

const VAULT_MARKERS: &[&str] = &[
    ".smart-vault.json",
    ".memvault.json",
    ".claude-obsidian.json",
    ".vault-meta",
    "wiki/index.md",
];

/// Returns true when `dir` looks like a `smart-vault` / `memvault` / `claude-obsidian` vault root.
#[must_use]
pub fn has_vault_markers(dir: &Path) -> bool {
    VAULT_MARKERS.iter().any(|marker| dir.join(marker).exists())
}

/// Resolves the vault root from `--vault`, env vars, or by walking up from CWD.
pub fn resolve_vault_root(
    explicit: Option<&Path>,
    allow_uninitialized: bool,
) -> Result<VaultSelection> {
    if let Some(path) = explicit {
        return resolve_explicit(path, "flag:--vault", allow_uninitialized);
    }

    for env_key in ["SMART_VAULT_ROOT", "MEMVAULT_ROOT", "CLAUDE_OBSIDIAN_VAULT"] {
        if let Ok(val) = env::var(env_key) {
            let trimmed = val.trim();
            if !trimmed.is_empty() {
                return resolve_explicit(
                    Path::new(trimmed),
                    &format!("env:{env_key}"),
                    allow_uninitialized,
                );
            }
        }
    }

    let cwd = env::current_dir()?;
    let mut cursor: Option<&Path> = Some(cwd.as_path());
    while let Some(dir) = cursor {
        if has_vault_markers(dir) {
            let canonical = fs::canonicalize(dir)?;
            return Ok(VaultSelection {
                root: canonical,
                source: "auto:cwd-walk".to_string(),
                initialized: true,
            });
        }
        cursor = dir.parent();
    }

    if allow_uninitialized {
        let canonical = fs::canonicalize(&cwd)?;
        return Ok(VaultSelection {
            root: canonical,
            source: "default:cwd".to_string(),
            initialized: false,
        });
    }

    Err(MemVaultError::VaultNotFound(format!(
        "no vault markers (.smart-vault.json, .memvault.json, .claude-obsidian.json, .vault-meta, wiki/index.md) found from {}",
        cwd.display()
    )))
}

fn resolve_explicit(
    path: &Path,
    source: &str,
    allow_uninitialized: bool,
) -> Result<VaultSelection> {
    if path.as_os_str().is_empty() {
        return Err(MemVaultError::InvalidPath("vault path must not be empty".to_string()));
    }

    match fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(err) if err.kind() == ErrorKind::NotFound => {
            if allow_uninitialized {
                // Allow non-existent target dir only when parent exists or can be created during init
                let abs = if path.is_absolute() {
                    path.to_path_buf()
                } else {
                    env::current_dir()?.join(path)
                };
                return Ok(VaultSelection {
                    root: abs,
                    source: source.to_string(),
                    initialized: false,
                });
            }
            return Err(MemVaultError::VaultNotFound(format!("{} does not exist", path.display())));
        }
        Err(err) => return Err(err.into()),
    }

    assert_unaliased_directory(path)?;
    let canonical = fs::canonicalize(path)?;
    let initialized = has_vault_markers(&canonical);
    if !initialized && !allow_uninitialized {
        return Err(MemVaultError::VaultNotFound(format!(
            "{} exists but is not an initialized vault",
            canonical.display()
        )));
    }

    Ok(VaultSelection { root: canonical, source: source.to_string(), initialized })
}

/// Ensures `path` exists as a real non-symlink directory.
pub fn assert_unaliased_directory(path: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        return Err(MemVaultError::SymlinkEscape(format!("{} is a symlink", path.display())));
    }
    if !meta.is_dir() {
        return Err(MemVaultError::InvalidPath(format!("{} is not a directory", path.display())));
    }
    Ok(())
}

/// Normalizes a relative vault path and checks every existing ancestor against symlink escapes.
pub fn safe_vault_join(vault_root: &Path, relative: &Path) -> Result<PathBuf> {
    if relative.to_string_lossy().contains('\0') {
        return Err(MemVaultError::InvalidPath("relative path contains NUL byte".to_string()));
    }
    if relative.is_absolute() {
        return Err(MemVaultError::InvalidPath(format!(
            "expected vault-relative path, got {}",
            relative.display()
        )));
    }

    let mut clean = PathBuf::new();
    for component in relative.components() {
        match component {
            Component::Normal(part) => clean.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(MemVaultError::SymlinkEscape(format!(
                    "path traversal component in {}",
                    relative.display()
                )));
            }
        }
    }

    if clean.as_os_str().is_empty() {
        return Err(MemVaultError::InvalidPath("empty relative path".to_string()));
    }

    assert_unaliased_directory(vault_root)?;
    let canonical_root = fs::canonicalize(vault_root)?;
    let mut cursor = canonical_root.clone();
    for component in clean.components() {
        cursor.push(component);
        match fs::symlink_metadata(&cursor) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(MemVaultError::SymlinkEscape(format!(
                    "symlink component at {}",
                    cursor.display()
                )));
            }
            Ok(_) => {}
            Err(err) if err.kind() == ErrorKind::NotFound => break,
            Err(err) => return Err(err.into()),
        }
    }

    Ok(canonical_root.join(clean))
}

/// Converts a path inside `vault_root` to forward-slash POSIX string for deterministic output.
#[must_use]
pub fn to_posix_rel(vault_root: &Path, path: &Path) -> String {
    let rel = if let Ok(stripped) = path.strip_prefix(vault_root) {
        stripped
    } else if let Ok(canon_root) = fs::canonicalize(vault_root) {
        path.strip_prefix(&canon_root).unwrap_or(path)
    } else {
        path
    };
    rel.components()
        .filter(|c| !matches!(c, Component::RootDir | Component::Prefix(_)))
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}
