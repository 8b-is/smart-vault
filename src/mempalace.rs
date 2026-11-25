use std::env;
use std::fs;
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::error::{MemVaultError, Result};
use crate::paths::assert_unaliased_directory;

const FAST_TIMEOUT: Duration = Duration::from_secs(10);
const HEAVY_TIMEOUT: Duration = Duration::from_secs(45);
const HOOK_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_SUBPROCESS_OUTPUT_BYTES: u64 = 16 * 1024 * 1024;

/// Probe summary for locally installed `mempalace` binaries and palace store.
#[derive(Debug, Clone, Serialize)]
pub struct MemPalaceInfo {
    pub available: bool,
    pub binary_path: Option<String>,
    pub mcp_binary_path: Option<String>,
    pub light_mcp_binary_path: Option<String>,
    pub palace_dir: Option<String>,
    pub palace_exists: bool,
}

/// Output payload returned by `mempalace` CLI operations.
#[derive(Debug, Clone, Serialize)]
pub struct PalaceOutput {
    pub command: String,
    pub stdout: String,
    pub stderr: String,
    pub elapsed_ms: u128,
}

/// Locates a `mempalace*` executable in `PATH`, `~/.local/bin/`, or `~/.cargo/bin/`.
#[must_use]
pub fn find_mempalace_binary(name: &str) -> Option<PathBuf> {
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\', '\0']) {
        return None;
    }
    if let Ok(path_var) = env::var("PATH") {
        for dir in env::split_paths(&path_var) {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    if let Ok(home) = env::var("HOME") {
        for rel in [".local/bin", ".cargo/bin"] {
            let candidate = PathBuf::from(&home).join(rel).join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Resolves the default MemPalace directory (`~/.config/mempalace/palace` or `~/.mempalace/palace`).
#[must_use]
pub fn default_palace_dir() -> Option<PathBuf> {
    let home = env::var("HOME").ok()?;
    let xdg = PathBuf::from(&home).join(".config/mempalace/palace");
    if xdg.exists() {
        return Some(xdg);
    }
    let legacy = PathBuf::from(&home).join(".mempalace/palace");
    if legacy.exists() {
        return Some(legacy);
    }
    Some(xdg)
}

/// Performs a zero-overhead probe of `mempalace` binaries and palace store on disk.
#[must_use]
pub fn probe_mempalace() -> MemPalaceInfo {
    let bin = find_mempalace_binary("mempalace");
    let mcp_bin = find_mempalace_binary("mempalace-mcp");
    let light_bin = find_mempalace_binary("mempalace-light-mcp");
    let palace_dir = default_palace_dir();
    let palace_exists = palace_dir.as_ref().is_some_and(|p| p.is_dir());

    MemPalaceInfo {
        available: bin.is_some(),
        binary_path: bin.map(|p| p.display().to_string()),
        mcp_binary_path: mcp_bin.map(|p| p.display().to_string()),
        light_mcp_binary_path: light_bin.map(|p| p.display().to_string()),
        palace_dir: palace_dir.map(|p| p.display().to_string()),
        palace_exists,
    }
}

fn validate_flag_value(label: &str, value: &str) -> Result<()> {
    if value.trim().is_empty()
        || value.starts_with('-')
        || value.chars().any(|c| c.is_ascii_control())
    {
        return Err(MemVaultError::Validation(format!(
            "invalid mempalace {label}: must be non-empty, not start with '-', and contain no control characters"
        )));
    }
    Ok(())
}

fn validate_identifier_token(label: &str, value: &str) -> Result<()> {
    if value.is_empty()
        || value.starts_with('-')
        || !value.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(MemVaultError::Validation(format!(
            "invalid mempalace {label} token '{value}'"
        )));
    }
    Ok(())
}

fn read_bounded_pipe<R: Read>(pipe: Option<R>, max_bytes: u64) -> String {
    let mut buf = Vec::new();
    if let Some(p) = pipe {
        let _ = p.take(max_bytes).read_to_end(&mut buf);
    }
    String::from_utf8_lossy(&buf).into_owned()
}

fn run_mempalace_cmd(
    args: &[&str],
    stdin_data: Option<&str>,
    timeout: Duration,
) -> Result<PalaceOutput> {
    let start = Instant::now();
    let bin = find_mempalace_binary("mempalace").ok_or_else(|| {
        MemVaultError::Validation(
            "mempalace binary not found in PATH or ~/.local/bin/mempalace".to_string(),
        )
    })?;

    let mut cmd = Command::new(bin);
    cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
    if stdin_data.is_some() {
        cmd.stdin(Stdio::piped());
    } else {
        cmd.stdin(Stdio::null());
    }

    let mut child = cmd.spawn()?;
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();

    let stdout_handle =
        thread::spawn(move || read_bounded_pipe(stdout_pipe, MAX_SUBPROCESS_OUTPUT_BYTES));
    let stderr_handle =
        thread::spawn(move || read_bounded_pipe(stderr_pipe, MAX_SUBPROCESS_OUTPUT_BYTES));

    let stdin_handle = stdin_data.and_then(|input| {
        child.stdin.take().map(|mut stdin_pipe| {
            let owned = input.as_bytes().to_vec();
            thread::spawn(move || {
                let _ = stdin_pipe.write_all(&owned);
            })
        })
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(st) = child.try_wait()? {
            break st;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(MemVaultError::Validation(format!(
                "mempalace {} timed out after {}ms",
                args.join(" "),
                timeout.as_millis()
            )));
        }
        thread::sleep(Duration::from_millis(25));
    };

    if let Some(handle) = stdin_handle {
        let _ = handle.join();
    }
    let stdout = stdout_handle.join().unwrap_or_default();
    let stderr = stderr_handle.join().unwrap_or_default();

    if !status.success() {
        return Err(MemVaultError::Validation(format!(
            "mempalace {} exited with {}: {}",
            args.join(" "),
            status,
            stderr.trim()
        )));
    }

    Ok(PalaceOutput {
        command: format!("mempalace {}", args.join(" ")),
        stdout,
        stderr,
        elapsed_ms: start.elapsed().as_millis(),
    })
}

/// Checks MemPalace health (`repair-status` by default, or full `status` if `full == true`).
pub fn status(full: bool) -> Result<PalaceOutput> {
    if full {
        run_mempalace_cmd(&["status"], None, HEAVY_TIMEOUT)
    } else {
        run_mempalace_cmd(&["repair-status"], None, FAST_TIMEOUT)
    }
}

/// Loads L0 + L1 MemPalace wake-up context, optionally scoped to a `wing`.
pub fn wake_up(wing: Option<&str>) -> Result<PalaceOutput> {
    match wing {
        Some(w) => {
            validate_flag_value("wing", w)?;
            run_mempalace_cmd(&["wake-up", "--wing", w], None, HEAVY_TIMEOUT)
        }
        None => run_mempalace_cmd(&["wake-up"], None, HEAVY_TIMEOUT),
    }
}

/// Searches MemPalace verbatim drawers via `mempalace search`.
pub fn search(
    query: &str,
    wing: Option<&str>,
    room: Option<&str>,
    results: Option<usize>,
) -> Result<PalaceOutput> {
    if query.contains('\0') {
        return Err(MemVaultError::Validation(
            "mempalace search query contains NUL byte".to_string(),
        ));
    }
    let mut args = vec!["search".to_string()];
    if let Some(w) = wing {
        validate_flag_value("wing", w)?;
        args.push("--wing".to_string());
        args.push(w.to_string());
    }
    if let Some(r) = room {
        validate_flag_value("room", r)?;
        args.push("--room".to_string());
        args.push(r.to_string());
    }
    if let Some(n) = results {
        args.push("--results".to_string());
        args.push(n.to_string());
    }
    args.push("--".to_string());
    args.push(query.to_string());
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    run_mempalace_cmd(&arg_refs, None, HEAVY_TIMEOUT)
}

/// Mines `<vault_root>/wiki` into MemPalace.
pub fn mine_vault(vault_root: &Path, wing: Option<&str>, dry_run: bool) -> Result<PalaceOutput> {
    assert_unaliased_directory(vault_root)?;
    let wiki_dir = vault_root.join("wiki");
    let target = match fs::symlink_metadata(&wiki_dir) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(MemVaultError::SymlinkEscape(format!(
                "refusing to mine symlinked wiki directory {}",
                wiki_dir.display()
            )));
        }
        Ok(meta) if meta.is_dir() => fs::canonicalize(&wiki_dir)?,
        Ok(_) => fs::canonicalize(vault_root)?,
        Err(err) if err.kind() == ErrorKind::NotFound => fs::canonicalize(vault_root)?,
        Err(err) => return Err(err.into()),
    };

    let target_str = target.display().to_string();
    let mut args = vec!["mine"];
    if let Some(w) = wing {
        validate_flag_value("wing", w)?;
        args.push("--wing");
        args.push(w);
    }
    if dry_run {
        args.push("--dry-run");
    }
    args.push("--");
    args.push(&target_str);
    run_mempalace_cmd(&args, None, HEAVY_TIMEOUT)
}

/// Exports the MemPalace tree into Markdown files under `out_dir`.
pub fn export_palace(out_dir: &Path) -> Result<PalaceOutput> {
    if out_dir.as_os_str().is_empty() || out_dir.to_string_lossy().contains('\0') {
        return Err(MemVaultError::InvalidPath("invalid export output directory".to_string()));
    }
    if let Ok(meta) = fs::symlink_metadata(out_dir) {
        if meta.file_type().is_symlink() {
            return Err(MemVaultError::SymlinkEscape(format!(
                "refusing to export into symlink {}",
                out_dir.display()
            )));
        }
    }
    let out_str = out_dir.display().to_string();
    run_mempalace_cmd(&["export", "--", &out_str], None, HEAVY_TIMEOUT)
}

/// Controls the warm MemPalace background daemon (`status`, `start`, `stop`).
pub fn daemon(action: &str) -> Result<PalaceOutput> {
    if !matches!(action, "status" | "start" | "stop" | "jobs") {
        return Err(MemVaultError::Validation(format!(
            "unsupported daemon action '{action}' (expected status|start|stop|jobs)"
        )));
    }
    run_mempalace_cmd(&["daemon", action], None, FAST_TIMEOUT)
}

/// Forwards a lifecycle hook event to `mempalace hook run --hook <name> --harness <harness>`.
pub fn forward_hook(hook_name: &str, harness: &str, stdin_json: &str) -> Result<String> {
    validate_identifier_token("hook_name", hook_name)?;
    validate_identifier_token("harness", harness)?;
    let out = run_mempalace_cmd(
        &["hook", "run", "--hook", hook_name, "--harness", harness],
        Some(stdin_json),
        HOOK_TIMEOUT,
    )?;
    Ok(out.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_option_injection_and_path_traversal_in_mempalace_helpers() {
        assert!(find_mempalace_binary("../evil").is_none());
        assert!(find_mempalace_binary("a/b").is_none());
        assert!(find_mempalace_binary("..").is_none());

        assert!(matches!(wake_up(Some("--backend=evil")), Err(MemVaultError::Validation(_))));
        assert!(matches!(search("ok", Some("-x"), None, None), Err(MemVaultError::Validation(_))));
        assert!(matches!(
            search("ok", None, Some("--room-flag"), None),
            Err(MemVaultError::Validation(_))
        ));
        assert!(matches!(
            search("bad\0query", None, None, None),
            Err(MemVaultError::Validation(_))
        ));
        assert!(matches!(
            forward_hook("--evil", "claude-code", "{}"),
            Err(MemVaultError::Validation(_))
        ));
        assert!(matches!(
            forward_hook("stop", "claude code;rm", "{}"),
            Err(MemVaultError::Validation(_))
        ));
    }
}
