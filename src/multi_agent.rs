use std::env;
use std::fs::{self, File};
use std::io::{ErrorKind, Read};
#[cfg(unix)]
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::error::{MemVaultError, Result};
use crate::mempalace::find_mempalace_binary;
use crate::signing::sign_binary;
use crate::transaction::{
    atomic_write_bytes_with_mode, read_bounded_bytes, read_bounded_string, MAX_SINGLE_FILE_BYTES,
};

const RESTRICTED_CONFIG_MODE: u32 = 0o600;
const BINARY_INSTALL_MODE: u32 = 0o755;
const MAX_BINARY_COPY_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
pub struct HostSetupAction {
    pub host: String,
    pub link_path: String,
    pub target_path: String,
    pub applied: bool,
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct HostSetupReport {
    pub dry_run: bool,
    pub actions: Vec<HostSetupAction>,
    pub mcp_snippet: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct GlobalActionItem {
    pub step: String,
    pub target: String,
    pub status: String,
    pub applied: bool,
    pub backup: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GlobalSetupReport {
    pub dry_run: bool,
    pub home: String,
    pub smart_vault_bin: String,
    pub sv_symlink: String,
    pub mempalace_mcp_bin: Option<String>,
    pub actions: Vec<GlobalActionItem>,
}

pub const SUPPORTED_HOSTS: &[&str] =
    &["claude", "codex", "opencode", "gemini", "antigravity", "cursor", "windsurf", "zcode"];

fn resolve_home(home_override: Option<&Path>) -> Result<PathBuf> {
    let home = match home_override {
        Some(h) => h.to_path_buf(),
        None => {
            let val = env::var("HOME").map_err(|_| {
                MemVaultError::Validation("HOME environment variable is unset".to_string())
            })?;
            PathBuf::from(val)
        }
    };
    let s = home.to_string_lossy();
    if s.trim().is_empty() || s.chars().any(|c| c.is_ascii_control()) {
        return Err(MemVaultError::Validation(
            "HOME path is empty or contains invalid control characters".to_string(),
        ));
    }
    Ok(home)
}

fn shell_single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn read_binary_bounded(src: &Path, max_bytes: u64) -> Result<Vec<u8>> {
    let file = File::open(src)?;
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(MemVaultError::InvalidPath(format!(
            "binary source {} is not a regular file",
            src.display()
        )));
    }
    if meta.len() > max_bytes {
        return Err(MemVaultError::Validation(format!(
            "binary source {} exceeds {} byte limit",
            src.display(),
            max_bytes
        )));
    }
    let cap = usize::try_from(meta.len().min(max_bytes)).unwrap_or(0);
    let mut buf = Vec::with_capacity(cap);
    file.take(max_bytes.saturating_add(1)).read_to_end(&mut buf)?;
    if buf.len() as u64 > max_bytes {
        return Err(MemVaultError::Validation(format!(
            "binary source {} exceeds {} byte limit",
            src.display(),
            max_bytes
        )));
    }
    Ok(buf)
}

/// Links `skills/` into one or all agent host directories (dry-run by default unless `apply == true`).
pub fn setup_hosts(
    repo_root: &Path,
    hosts: &[String],
    workspace: Option<&Path>,
    apply: bool,
) -> Result<HostSetupReport> {
    let skills_src = fs::canonicalize(repo_root.join("skills")).map_err(|_| {
        MemVaultError::InvalidPath(format!(
            "skills directory missing under {}",
            repo_root.display()
        ))
    })?;

    let expanded: Vec<String> = if hosts.iter().any(|h| h == "all") {
        SUPPORTED_HOSTS.iter().map(|s| (*s).to_string()).collect()
    } else {
        hosts.to_vec()
    };

    let home = resolve_home(None)?;

    let mut actions = Vec::new();

    for host in expanded {
        let link_dir = match host.as_str() {
            "claude" => home.join(".claude/skills/smart-vault"),
            "codex" => home.join(".codex/skills/smart-vault"),
            "opencode" => home.join(".config/opencode/skills/smart-vault"),
            "gemini" | "antigravity" => home.join(".gemini/config/skills/smart-vault"),
            "zcode" => home.join(".zcode/skills/smart-vault"),
            "cursor" => {
                let ws = workspace.ok_or_else(|| {
                    MemVaultError::Validation(
                        "--workspace is required for host 'cursor'".to_string(),
                    )
                })?;
                ws.join(".cursor/skills/smart-vault")
            }
            "windsurf" => {
                let ws = workspace.ok_or_else(|| {
                    MemVaultError::Validation(
                        "--workspace is required for host 'windsurf'".to_string(),
                    )
                })?;
                ws.join(".windsurf/skills/smart-vault")
            }
            other => {
                return Err(MemVaultError::Validation(format!(
                    "unsupported host '{other}' (expected one of: {}, all)",
                    SUPPORTED_HOSTS.join(", ")
                )));
            }
        };

        let status = link_skills_dir(&skills_src, &link_dir, apply)?;
        actions.push(HostSetupAction {
            host,
            link_path: link_dir.display().to_string(),
            target_path: skills_src.display().to_string(),
            applied: apply,
            status,
        });
    }

    let mcp_snippet = json!({
        "mcpServers": {
            "smart-vault": {
                "command": "smart-vault",
                "args": ["mcp"]
            }
        }
    });

    Ok(HostSetupReport { dry_run: !apply, actions, mcp_snippet })
}

fn link_skills_dir(skills_src: &Path, link_dir: &Path, apply: bool) -> Result<String> {
    if !apply {
        return Ok(if link_dir.exists() || link_dir.symlink_metadata().is_ok() {
            "would-replace-symlink".to_string()
        } else {
            "would-link".to_string()
        });
    }

    if let Some(parent) = link_dir.parent() {
        fs::create_dir_all(parent)?;
    }
    if let Ok(meta) = link_dir.symlink_metadata() {
        if meta.file_type().is_symlink() {
            fs::remove_file(link_dir)?;
        } else {
            return Err(MemVaultError::InvalidPath(format!(
                "refusing to overwrite existing non-symlink {}",
                link_dir.display()
            )));
        }
    }
    #[cfg(unix)]
    symlink(skills_src, link_dir)?;
    Ok("linked".to_string())
}

/// Automated global installer for macOS M1 + Antigravity IDE/CLI + MemPalace + MemVault.
/// Dry-run by default (`apply == false`); backs up modified JSON configs with `.bak` before atomic write.
pub fn setup_global_m1(
    repo_root: &Path,
    binary_src: Option<&Path>,
    home_override: Option<&Path>,
    light_mcp: bool,
    apply: bool,
) -> Result<GlobalSetupReport> {
    let home = resolve_home(home_override)?;

    let skills_src = fs::canonicalize(repo_root.join("skills")).map_err(|_| {
        MemVaultError::InvalidPath(format!(
            "skills directory missing under {}",
            repo_root.display()
        ))
    })?;

    let dest_bin = home.join(".local/bin/smart-vault");
    let sv_link = home.join(".local/bin/sv");
    let src_bin = binary_src.map(Path::to_path_buf).or_else(|| env::current_exe().ok());

    let palace_mcp_name = if light_mcp { "mempalace-light-mcp" } else { "mempalace-mcp" };
    let mempalace_mcp_bin = find_mempalace_binary(palace_mcp_name)
        .or_else(|| find_mempalace_binary("mempalace-mcp"))
        .map(|p| p.display().to_string());

    let mut actions = Vec::new();

    // 1. Install `smart-vault` binary into ~/.local/bin/smart-vault, sign with codesign, and link `sv`
    let bin_status = match src_bin {
        Some(ref src) if src.is_file() => {
            let same_file = fs::canonicalize(src).ok() == fs::canonicalize(&dest_bin).ok();
            if same_file && !apply {
                "already-in-place".to_string()
            } else if apply {
                if !same_file {
                    let bytes = read_binary_bounded(src, MAX_BINARY_COPY_BYTES)?;
                    atomic_write_bytes_with_mode(&dest_bin, &bytes, Some(BINARY_INSTALL_MODE))?;
                }
                let sign_rep = sign_binary(&dest_bin, None, true)?;
                let _ = link_skills_dir(&dest_bin, &sv_link, true);
                format!(
                    "{}+signed({})",
                    if same_file { "already-in-place" } else { "installed" },
                    &sign_rep.sha256[..12]
                )
            } else {
                format!("would-copy-and-sign-from:{}", src.display())
            }
        }
        _ => "skipped-no-source-binary".to_string(),
    };
    actions.push(GlobalActionItem {
        step: "install_and_sign_binary".to_string(),
        target: dest_bin.display().to_string(),
        status: bin_status,
        applied: apply,
        backup: None,
    });

    // 2. Idempotently merge `smart-vault` + `mempalace` into ~/.gemini/config/mcp_config.json (IDE + CLI)
    let mcp_cfg_path = home.join(".gemini/config/mcp_config.json");
    let mcp_backup =
        merge_mcp_config(&mcp_cfg_path, &dest_bin, mempalace_mcp_bin.as_deref(), apply)?;
    actions.push(GlobalActionItem {
        step: "merge_antigravity_mcp_config".to_string(),
        target: mcp_cfg_path.display().to_string(),
        status: if apply { "merged".to_string() } else { "would-merge".to_string() },
        applied: apply,
        backup: mcp_backup,
    });

    // 3. Idempotently merge global hooks into ~/.gemini/settings.json
    let settings_path = home.join(".gemini/settings.json");
    let settings_backup = merge_gemini_hooks(&settings_path, &dest_bin, apply)?;
    actions.push(GlobalActionItem {
        step: "merge_antigravity_global_hooks".to_string(),
        target: settings_path.display().to_string(),
        status: if apply { "merged".to_string() } else { "would-merge".to_string() },
        applied: apply,
        backup: settings_backup,
    });

    // 4. Link skills into ~/.gemini/config/skills/smart-vault (and clean legacy memvault symlink)
    let skill_link = home.join(".gemini/config/skills/smart-vault");
    let legacy_skill_link = home.join(".gemini/config/skills/memvault");
    if apply && legacy_skill_link.symlink_metadata().is_ok_and(|m| m.file_type().is_symlink()) {
        let _ = fs::remove_file(&legacy_skill_link);
    }
    let skill_status = link_skills_dir(&skills_src, &skill_link, apply)?;
    actions.push(GlobalActionItem {
        step: "link_antigravity_skills".to_string(),
        target: skill_link.display().to_string(),
        status: skill_status,
        applied: apply,
        backup: None,
    });

    Ok(GlobalSetupReport {
        dry_run: !apply,
        home: home.display().to_string(),
        smart_vault_bin: dest_bin.display().to_string(),
        sv_symlink: sv_link.display().to_string(),
        mempalace_mcp_bin,
        actions,
    })
}

fn backup_if_exists(path: &Path) -> Result<Option<String>> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(MemVaultError::SymlinkEscape(format!(
                "refusing to back up symlink {}",
                path.display()
            )));
        }
        Ok(meta) if meta.is_file() => {}
        Ok(_) => {
            return Err(MemVaultError::InvalidPath(format!(
                "{} is not a regular file",
                path.display()
            )));
        }
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    }

    let bak = path.with_extension("json.bak");
    match fs::symlink_metadata(&bak) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(MemVaultError::SymlinkEscape(format!(
                "refusing to use symlinked backup {}",
                bak.display()
            )));
        }
        Ok(_) => {}
        Err(err) if err.kind() == ErrorKind::NotFound => {
            let bytes = read_bounded_bytes(path, MAX_SINGLE_FILE_BYTES)?;
            atomic_write_bytes_with_mode(&bak, &bytes, Some(RESTRICTED_CONFIG_MODE))?;
        }
        Err(err) => return Err(err.into()),
    }
    Ok(Some(bak.display().to_string()))
}

fn read_json_object(path: &Path) -> Result<Map<String, Value>> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(MemVaultError::SymlinkEscape(format!(
                "refusing to read symlink {}",
                path.display()
            )));
        }
        Ok(meta) if meta.is_file() => {}
        Ok(_) => {
            return Err(MemVaultError::InvalidPath(format!(
                "{} is not a regular file",
                path.display()
            )));
        }
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Map::new()),
        Err(err) => return Err(err.into()),
    }
    let raw = read_bounded_string(path, MAX_SINGLE_FILE_BYTES)?;
    let val: Value = serde_json::from_str(&raw)?;
    val.as_object().cloned().ok_or_else(|| {
        MemVaultError::InvalidManifest(format!("{} must contain a JSON object", path.display()))
    })
}

fn merge_mcp_config(
    path: &Path,
    memvault_bin: &Path,
    mempalace_mcp_bin: Option<&str>,
    apply: bool,
) -> Result<Option<String>> {
    let mut root = read_json_object(path)?;
    let servers = root.entry("mcpServers".to_string()).or_insert_with(|| Value::Object(Map::new()));
    let servers_map = servers.as_object_mut().ok_or_else(|| {
        MemVaultError::InvalidManifest("mcpServers must be a JSON object".to_string())
    })?;

    servers_map.remove("memvault");
    servers_map.insert(
        "smart-vault".to_string(),
        json!({
            "command": memvault_bin.display().to_string(),
            "args": ["mcp"]
        }),
    );

    if let Some(mp_bin) = mempalace_mcp_bin {
        servers_map.insert(
            "mempalace".to_string(),
            json!({
                "command": mp_bin,
                "args": []
            }),
        );
    }

    if !apply {
        return Ok(None);
    }
    let backup = backup_if_exists(path)?;
    let formatted = format!("{}\n", serde_json::to_string_pretty(&Value::Object(root))?);
    atomic_write_bytes_with_mode(path, formatted.as_bytes(), Some(RESTRICTED_CONFIG_MODE))?;
    Ok(backup)
}

fn merge_gemini_hooks(path: &Path, memvault_bin: &Path, apply: bool) -> Result<Option<String>> {
    let mut root = read_json_object(path)?;
    let hooks = root.entry("hooks".to_string()).or_insert_with(|| Value::Object(Map::new()));
    let hooks_map = hooks.as_object_mut().ok_or_else(|| {
        MemVaultError::InvalidManifest("settings.json hooks must be a JSON object".to_string())
    })?;

    let quoted_bin = shell_single_quote(&memvault_bin.display().to_string());
    let mappings = [
        ("SessionStart", "session-start"),
        ("PreCompress", "pre-compact"),
        ("AfterAgent", "stop"),
        ("SessionEnd", "session-end"),
    ];

    for (gemini_event, subcmd) in mappings {
        let cmd_str = format!("{quoted_bin} hook {subcmd} --palace");
        let marker = format!("hook {subcmd}");
        let entries =
            hooks_map.entry(gemini_event.to_string()).or_insert_with(|| Value::Array(Vec::new()));
        let arr = entries.as_array_mut().ok_or_else(|| {
            MemVaultError::InvalidManifest(format!("hooks.{gemini_event} must be an array"))
        })?;

        // Drop legacy `memvault hook` entries when upgrading to `smart-vault`
        arr.retain(|entry| {
            !entry.get("hooks").and_then(Value::as_array).is_some_and(|inner| {
                inner.iter().any(|h| {
                    h.get("command")
                        .and_then(Value::as_str)
                        .is_some_and(|c| c.contains("/memvault") && c.contains("hook "))
                })
            })
        });

        let already_present = arr.iter().any(|entry| {
            entry.get("hooks").and_then(Value::as_array).is_some_and(|inner| {
                inner.iter().any(|h| {
                    h.get("command")
                        .and_then(Value::as_str)
                        .is_some_and(|c| c.contains("smart-vault") && c.contains(&marker))
                })
            })
        });

        if !already_present {
            arr.push(json!({
                "hooks": [
                    {
                        "type": "command",
                        "command": cmd_str
                    }
                ]
            }));
        }
    }

    if !apply {
        return Ok(None);
    }
    let backup = backup_if_exists(path)?;
    let formatted = format!("{}\n", serde_json::to_string_pretty(&Value::Object(root))?);
    atomic_write_bytes_with_mode(path, formatted.as_bytes(), Some(RESTRICTED_CONFIG_MODE))?;
    Ok(backup)
}
