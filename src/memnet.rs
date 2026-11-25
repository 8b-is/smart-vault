use std::fs;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::bm25::{query_index, SearchResponse};
use crate::error::{MemVaultError, Result};
use crate::mempalace::search as palace_search;
use crate::paths::assert_unaliased_directory;
use crate::smart_tree::find_st_binary;

const DEFAULT_MEMNET_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_SUBPROCESS_OUTPUT_BYTES: u64 = 16 * 1024 * 1024;

/// Result of querying the MemNet wave cache, local Rust BM25+ vault index, and optional MemPalace.
#[derive(Debug, Clone, Serialize)]
pub struct HybridQueryResponse {
    pub query: String,
    pub mesh_used: bool,
    pub mesh_hit: Option<Value>,
    pub mesh_error: Option<String>,
    pub palace_used: bool,
    pub palace_output: Option<String>,
    pub palace_error: Option<String>,
    pub vault: SearchResponse,
    pub total_elapsed_us: u128,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MemNetStatusSummary {
    pub available: bool,
    pub local_address: Option<u64>,
    pub node_name: Option<String>,
    pub tier: Option<String>,
    pub routes_up: usize,
    pub total_routes: usize,
    pub raw: Option<Value>,
}

/// Strips any leading non-JSON banner lines and parses the JSON object or array payload.
pub fn parse_json_from_stdout(raw: &str) -> Result<Value> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(MemVaultError::MemNet("empty stdout from st memnet".to_string()));
    }

    if let Ok(val) = serde_json::from_str::<Value>(trimmed) {
        return Ok(val);
    }

    let mut offset = 0usize;
    for line in trimmed.split_inclusive('\n') {
        let line_trim = line.trim_start();
        if line_trim.starts_with('{') || line_trim.starts_with('[') {
            let lead_ws = line.len() - line_trim.len();
            if let Ok(val) = serde_json::from_str::<Value>(&trimmed[offset + lead_ws..]) {
                return Ok(val);
            }
        }
        offset += line.len();
    }

    let obj_idx = trimmed.find('{');
    let arr_idx = trimmed.find('[');
    let start = match (obj_idx, arr_idx) {
        (Some(a), Some(b)) => a.min(b),
        (Some(a), None) => a,
        (None, Some(b)) => b,
        (None, None) => {
            return Err(MemVaultError::MemNet(format!(
                "no JSON payload found in stdout: {}",
                trimmed.lines().next().unwrap_or("")
            )));
        }
    };

    serde_json::from_str::<Value>(&trimmed[start..]).map_err(Into::into)
}

fn read_bounded_pipe<R: Read>(pipe: Option<R>, max_bytes: u64) -> String {
    let mut buf = Vec::new();
    if let Some(p) = pipe {
        let _ = p.take(max_bytes).read_to_end(&mut buf);
    }
    String::from_utf8_lossy(&buf).into_owned()
}

fn run_st_memnet_json(args: &[&str], timeout: Duration) -> Result<Value> {
    let bin = find_st_binary().ok_or_else(|| {
        MemVaultError::MemNet(
            "8b-is/smart-tree (`st`) binary not found in PATH or ~/.local/bin/st".to_string(),
        )
    })?;

    let mut child = Command::new(bin)
        .arg("--no-update-check")
        .arg("memnet")
        .arg("--json")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();

    let stdout_handle =
        thread::spawn(move || read_bounded_pipe(stdout_pipe, MAX_SUBPROCESS_OUTPUT_BYTES));
    let stderr_handle =
        thread::spawn(move || read_bounded_pipe(stderr_pipe, MAX_SUBPROCESS_OUTPUT_BYTES));

    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(st) = child.try_wait()? {
            break st;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(MemVaultError::MemNet(format!(
                "st memnet {} timed out after {}ms",
                args.join(" "),
                timeout.as_millis()
            )));
        }
        thread::sleep(Duration::from_millis(20));
    };

    let stdout = stdout_handle.join().unwrap_or_default();
    let stderr = stderr_handle.join().unwrap_or_default();

    if !status.success() {
        return Err(MemVaultError::MemNet(format!(
            "st memnet {} failed ({status}): {}",
            args.join(" "),
            stderr.trim()
        )));
    }

    parse_json_from_stdout(&stdout)
}

/// Returns live `st memnet status --json` data.
pub fn status() -> Result<Value> {
    run_st_memnet_json(&["status"], DEFAULT_MEMNET_TIMEOUT)
}

/// Safe non-failing probe for `smart-vault doctor` and hooks.
#[must_use]
pub fn probe_status() -> MemNetStatusSummary {
    match status() {
        Ok(val) => {
            let routes = val.get("routes").and_then(Value::as_array).map_or(&[][..], Vec::as_slice);
            let routes_up = routes
                .iter()
                .filter(|r| r.get("status").and_then(Value::as_str) == Some("Up"))
                .count();
            MemNetStatusSummary {
                available: true,
                local_address: val.get("local_address").and_then(Value::as_u64),
                node_name: val.get("node_name").and_then(Value::as_str).map(str::to_string),
                tier: val.get("tier").and_then(Value::as_str).map(str::to_string),
                routes_up,
                total_routes: routes.len(),
                raw: Some(val),
            }
        }
        Err(_) => MemNetStatusSummary {
            available: false,
            local_address: None,
            node_name: None,
            tier: None,
            routes_up: 0,
            total_routes: 0,
            raw: None,
        },
    }
}

/// Lists live mesh routes (`st memnet routes --json`).
pub fn routes() -> Result<Value> {
    run_st_memnet_json(&["routes"], DEFAULT_MEMNET_TIMEOUT)
}

/// Publishes the Obsidian vault folder onto MemNet (`st memnet share <vault> --quota-mb <mb> --json`).
pub fn share_vault(vault_root: &Path, quota_mb: u64) -> Result<Value> {
    assert_unaliased_directory(vault_root)?;
    let canonical = fs::canonicalize(vault_root)?;
    let vault_str = canonical.display().to_string();
    let quota_str = quota_mb.to_string();
    run_st_memnet_json(
        &["--quota-mb", &quota_str, "share", "--", &vault_str],
        DEFAULT_MEMNET_TIMEOUT,
    )
}

/// Queries the MemNet `MEM|8` wave cache (`st memnet ask <question> --json`).
pub fn ask(question: &str, timeout: Option<Duration>) -> Result<Value> {
    if question.contains('\0') {
        return Err(MemVaultError::Validation("memnet question contains NUL byte".to_string()));
    }
    run_st_memnet_json(&["ask", "--", question], timeout.unwrap_or(DEFAULT_MEMNET_TIMEOUT))
}

/// Anchors a Q&A packet into MemNet (`st memnet teach <question> <answer> --json`).
pub fn teach(question: &str, answer: &str) -> Result<Value> {
    if question.contains('\0') || answer.contains('\0') {
        return Err(MemVaultError::Validation(
            "memnet teach question/answer contains NUL byte".to_string(),
        ));
    }
    run_st_memnet_json(&["teach", "--", question, answer], DEFAULT_MEMNET_TIMEOUT)
}

/// Runs a unified query: checks MemNet wave cache (`use_mesh`), optional MemPalace verbatim drawers
/// (`use_palace`), and parallel BM25+ search across the local Obsidian vault.
pub fn hybrid_query(
    vault_root: &Path,
    query: &str,
    top_k: usize,
    use_mesh: bool,
    use_palace: bool,
    auto_build: bool,
) -> Result<HybridQueryResponse> {
    let start = Instant::now();
    let (mesh_hit, mesh_error) = if use_mesh {
        match ask(query, Some(Duration::from_millis(1500))) {
            Ok(val) => {
                let has_hit = val
                    .get("packet")
                    .and_then(|p| p.get("answer"))
                    .and_then(Value::as_str)
                    .is_some_and(|a| !a.is_empty());
                if has_hit {
                    (Some(val), None)
                } else {
                    (None, None)
                }
            }
            Err(err) => (None, Some(err.to_string())),
        }
    } else {
        (None, None)
    };

    let (palace_output, palace_error) = if use_palace {
        match palace_search(query, None, None, Some(top_k)) {
            Ok(out) => (Some(out.stdout), None),
            Err(err) => (None, Some(err.to_string())),
        }
    } else {
        (None, None)
    };

    let vault = query_index(vault_root, query, top_k, auto_build)?;

    Ok(HybridQueryResponse {
        query: query.to_string(),
        mesh_used: use_mesh,
        mesh_hit,
        mesh_error,
        palace_used: use_palace,
        palace_output,
        palace_error,
        vault,
        total_elapsed_us: start.elapsed().as_micros(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_banner_lines_before_json() {
        let raw = "Smart Tree v24.4.1 available\n\n{\"local_address\":42,\"routes\":[]}\n";
        let val = parse_json_from_stdout(raw).expect("should parse json after banner");
        assert_eq!(val["local_address"], 42);
    }

    #[test]
    fn parses_json_array_with_prefix() {
        let raw = "banner line\n[{\"node_name\":\"en0\"}]\n";
        let val = parse_json_from_stdout(raw).expect("should parse json array");
        assert_eq!(val[0]["node_name"], "en0");
    }

    #[test]
    fn ignores_bracketed_banner_before_json() {
        let raw = "[INFO] MemNet mesh starting\n{\"local_address\":42}\n";
        let val = parse_json_from_stdout(raw).expect("should skip [INFO] banner");
        assert_eq!(val["local_address"], 42);
    }

    #[test]
    fn rejects_nul_in_ask_and_teach() {
        assert!(matches!(ask("bad\0q", None), Err(MemVaultError::Validation(_))));
        assert!(matches!(teach("ok", "bad\0a"), Err(MemVaultError::Validation(_))));
    }
}
