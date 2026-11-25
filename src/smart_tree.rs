use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use rayon::prelude::*;
use serde::Serialize;
use walkdir::WalkDir;

use crate::error::{MemVaultError, Result};
use crate::paths::to_posix_rel;

/// Information about the detected `8b-is/smart-tree` (`st`) binary.
#[derive(Debug, Clone, Serialize)]
pub struct SmartTreeInfo {
    pub available: bool,
    pub binary_path: Option<String>,
    pub version_line: Option<String>,
}

/// Output from rendering a vault tree via `st` or built-in Rust fallback.
#[derive(Debug, Clone, Serialize)]
pub struct TreeOutput {
    pub engine: String,
    pub mode: String,
    pub vault_root: String,
    pub output: String,
}

/// Output from syncing a vault into `MEM|8` waves via `st remember`.
#[derive(Debug, Clone, Serialize)]
pub struct RememberOutput {
    pub vault_root: String,
    pub level: String,
    pub check_only: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Locates the `st` (`8b-is/smart-tree`) executable in `PATH` or `~/.local/bin/st`.
#[must_use]
pub fn find_st_binary() -> Option<PathBuf> {
    if let Ok(path_var) = env::var("PATH") {
        for dir in env::split_paths(&path_var) {
            let candidate = dir.join("st");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    if let Ok(home) = env::var("HOME") {
        let local_st = PathBuf::from(&home).join(".local/bin/st");
        if local_st.is_file() {
            return Some(local_st);
        }
        let cargo_st = PathBuf::from(&home).join(".cargo/bin/st");
        if cargo_st.is_file() {
            return Some(cargo_st);
        }
    }
    None
}

/// Probes `st --version` and returns availability metadata.
#[must_use]
pub fn probe_smart_tree() -> SmartTreeInfo {
    let Some(bin) = find_st_binary() else {
        return SmartTreeInfo { available: false, binary_path: None, version_line: None };
    };

    let version_line = Command::new(&bin).arg("--version").output().ok().and_then(|out| {
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        parse_version_line(&text)
    });

    SmartTreeInfo { available: true, binary_path: Some(bin.display().to_string()), version_line }
}

fn parse_version_line(stdout: &str) -> Option<String> {
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(pos) = trimmed.find("Smart Tree v") {
            let candidate = trimmed[pos..].trim();
            if !candidate.contains("is available") {
                return Some(candidate.to_string());
            }
        }
    }
    stdout.lines().map(str::trim).find(|l| !l.is_empty()).map(ToString::to_string)
}

/// Runs `st --no-update-check --mode <mode> --depth <depth> <vault_root>` with pure-Rust fallback.
pub fn run_tree(vault_root: &Path, mode: &str, depth: Option<usize>) -> Result<TreeOutput> {
    if let Some(bin) = find_st_binary() {
        let mut cmd = Command::new(&bin);
        cmd.arg("--no-update-check").arg("--mode").arg(mode);
        if let Some(d) = depth {
            cmd.arg("--depth").arg(d.to_string());
        }
        cmd.arg(vault_root);

        let output = cmd.output()?;
        if output.status.success() {
            return Ok(TreeOutput {
                engine: "smart-tree".to_string(),
                mode: mode.to_string(),
                vault_root: vault_root.display().to_string(),
                output: String::from_utf8_lossy(&output.stdout).into_owned(),
            });
        }
    }

    // Ultra-fast built-in pure-Rust fallback tree when `st` is absent or declined
    let max_depth = depth.unwrap_or(4);
    let mut entries: Vec<(String, bool, u64)> = WalkDir::new(vault_root)
        .max_depth(max_depth)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            e.depth() == 0 || (name != ".git" && name != "target" && name != "node_modules")
        })
        .flatten()
        .filter(|e| e.depth() > 0)
        .collect::<Vec<_>>()
        .into_par_iter()
        .map(|entry| {
            let rel = to_posix_rel(vault_root, entry.path());
            let is_dir = entry.file_type().is_dir();
            let size = if is_dir { 0 } else { fs::metadata(entry.path()).map_or(0, |m| m.len()) };
            (rel, is_dir, size)
        })
        .collect();

    entries.sort_unstable_by(|a, b| a.0.cmp(&b.0));

    let mut rendered = String::with_capacity(entries.len() * 32);
    for (rel, is_dir, size) in entries {
        if is_dir {
            let _ = writeln!(rendered, "📁 {rel}/");
        } else {
            let _ = writeln!(rendered, "📄 {rel} ({size} B)");
        }
    }

    Ok(TreeOutput {
        engine: "smart-vault-fallback".to_string(),
        mode: mode.to_string(),
        vault_root: vault_root.display().to_string(),
        output: rendered,
    })
}

/// Runs `st --no-update-check remember <vault_root> --level <level> --out <vault>/.vault-meta/vault.m8 [--check] [--query <q>] [--force]`.
pub fn run_remember(
    vault_root: &Path,
    level: &str,
    check_only: bool,
    query: Option<&str>,
    force: bool,
) -> Result<RememberOutput> {
    let bin = find_st_binary().ok_or_else(|| {
        MemVaultError::SmartTree(
            "8b-is/smart-tree (`st`) binary not found in PATH or ~/.local/bin/st".to_string(),
        )
    })?;

    let meta_dir = vault_root.join(".vault-meta");
    if !meta_dir.exists() {
        fs::create_dir_all(&meta_dir)?;
    }
    let m8_out = meta_dir.join("vault.m8");

    let mut cmd = Command::new(bin);
    cmd.args(["--no-update-check", "remember"])
        .arg(vault_root)
        .arg("--level")
        .arg(level)
        .arg("--out")
        .arg(&m8_out);
    if check_only {
        cmd.arg("--check");
    }
    if force {
        cmd.arg("--force");
    }
    if let Some(q) = query {
        cmd.arg("--query").arg(q);
    }

    let output = cmd.output()?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if !output.status.success() {
        return Err(MemVaultError::SmartTree(format!(
            "st remember exited with {}: {}",
            output.status,
            stderr.trim()
        )));
    }

    Ok(RememberOutput {
        vault_root: vault_root.display().to_string(),
        level: level.to_string(),
        check_only,
        stdout,
        stderr,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_smart_tree_version_across_banners() {
        let output = "Smart Tree v24.4.1 is available (installed: v23.0.0).\n\n🌳 Smart Tree v23.0.0\n🔧 Target: aarch64\n";
        assert_eq!(parse_version_line(output).as_deref(), Some("Smart Tree v23.0.0"));
    }

    #[test]
    fn falls_back_to_first_non_empty_line() {
        assert_eq!(parse_version_line("\n  st 24.4.1  \n").as_deref(), Some("st 24.4.1"));
        assert_eq!(parse_version_line("   \n  ").as_deref(), None);
    }
}
