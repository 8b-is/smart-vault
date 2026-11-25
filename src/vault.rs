use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use chrono::Utc;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::error::{MemVaultError, Result};
use crate::markdown::parse_markdown;
use crate::memnet::{probe_status, MemNetStatusSummary};
use crate::mempalace::{probe_mempalace, MemPalaceInfo};
use crate::paths::{assert_unaliased_directory, safe_vault_join, to_posix_rel, VaultSelection};
use crate::smart_tree::{probe_smart_tree, SmartTreeInfo};
use crate::transaction::{
    apply_bundle, count_incomplete_journals, inspect_bundle, FileOp, TransactionBundle,
};

const TEMPLATE_GITIGNORE: &str = include_str!("../templates/vault/.gitignore");
const TEMPLATE_APP_JSON: &str = include_str!("../templates/vault/.obsidian/app.json");
const TEMPLATE_APPEARANCE_JSON: &str = include_str!("../templates/vault/.obsidian/appearance.json");
const TEMPLATE_GRAPH_JSON: &str = include_str!("../templates/vault/.obsidian/graph.json");
const TEMPLATE_COLORS_CSS: &str =
    include_str!("../templates/vault/.obsidian/snippets/vault-colors.css");
const TEMPLATE_RAW_MANIFEST: &str = include_str!("../templates/vault/.raw/.manifest.json");
const TEMPLATE_INDEX_MD: &str = include_str!("../templates/vault/wiki/index.md");
const TEMPLATE_OVERVIEW_MD: &str = include_str!("../templates/vault/wiki/overview.md");
const TEMPLATE_HOT_MD: &str = include_str!("../templates/vault/wiki/hot.md");
const TEMPLATE_LOG_MD: &str = include_str!("../templates/vault/wiki/log.md");

pub const VALID_MODES: &[&str] = &["standard", "zettel", "lyt", "para"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultConfig {
    pub schema: String,
    pub role: String,
    pub vault: String,
    pub source_inbox: String,
    pub legacy_raw: String,
    #[serde(default = "default_mode")]
    pub mode: String,
    #[serde(default)]
    pub smart_tree_enabled: bool,
    #[serde(default)]
    pub memnet_enabled: bool,
}

fn default_mode() -> String {
    "standard".to_string()
}

#[derive(Debug, Clone, Serialize)]
pub struct ScanReport {
    pub vault_root: String,
    pub mode: String,
    pub markdown_pages: usize,
    pub inbox_items: usize,
    pub raw_items: usize,
    pub total_files: usize,
    pub total_bytes: u64,
    pub folders: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DoctorReport {
    pub ok: bool,
    pub vault_root: String,
    pub selection_source: String,
    pub initialized: bool,
    pub metadata_writable: bool,
    pub incomplete_journals: usize,
    pub mode: String,
    pub smart_tree: SmartTreeInfo,
    pub memnet: MemNetStatusSummary,
    pub mempalace: MemPalaceInfo,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModeReport {
    pub vault_root: String,
    pub mode: String,
    pub applied: bool,
}

/// Builds a preview or applies initial vault scaffolding (`init`) or non-destructive `adopt`.
pub fn init_or_adopt(
    vault_root: &Path,
    mode: &str,
    adopt: bool,
    apply: bool,
    approved_sha256: Option<&str>,
) -> Result<serde_json::Value> {
    if !VALID_MODES.contains(&mode) {
        return Err(MemVaultError::Validation(format!(
            "unsupported methodology mode '{mode}' (expected one of: {})",
            VALID_MODES.join(", ")
        )));
    }

    if !adopt
        && (vault_root.join(".smart-vault.json").exists()
            || vault_root.join(".memvault.json").exists())
    {
        return Err(MemVaultError::AlreadyInitialized(vault_root.display().to_string()));
    }

    if apply && !vault_root.exists() {
        fs::create_dir_all(vault_root)?;
    }

    let today = Utc::now().format("%Y-%m-%d").to_string();
    let cfg = VaultConfig {
        schema: "smart-vault.workspace.v1".to_string(),
        role: "vault".to_string(),
        vault: ".".to_string(),
        source_inbox: "inbox".to_string(),
        legacy_raw: ".raw".to_string(),
        mode: mode.to_string(),
        smart_tree_enabled: true,
        memnet_enabled: true,
    };
    let cfg_json = serde_json::to_string_pretty(&cfg)?;

    let mut candidates: Vec<(&str, String)> = vec![
        (".smart-vault.json", format!("{cfg_json}\n")),
        (".gitignore", TEMPLATE_GITIGNORE.to_string()),
        (".obsidian/app.json", TEMPLATE_APP_JSON.to_string()),
        (".obsidian/appearance.json", TEMPLATE_APPEARANCE_JSON.to_string()),
        (".obsidian/graph.json", TEMPLATE_GRAPH_JSON.to_string()),
        (".obsidian/snippets/vault-colors.css", TEMPLATE_COLORS_CSS.to_string()),
        (".raw/.manifest.json", TEMPLATE_RAW_MANIFEST.to_string()),
        ("inbox/.gitkeep", String::new()),
        ("wiki/index.md", TEMPLATE_INDEX_MD.replace("{{generated_date}}", &today)),
        ("wiki/overview.md", TEMPLATE_OVERVIEW_MD.replace("{{generated_date}}", &today)),
        ("wiki/hot.md", TEMPLATE_HOT_MD.replace("{{generated_date}}", &today)),
        ("wiki/log.md", TEMPLATE_LOG_MD.replace("{{generated_date}}", &today)),
    ];

    let mut ops = vec![
        FileOp::Mkdir { path: ".vault-meta".to_string() },
        FileOp::Mkdir { path: ".raw".to_string() },
        FileOp::Mkdir { path: "inbox".to_string() },
        FileOp::Mkdir { path: "wiki/concepts".to_string() },
        FileOp::Mkdir { path: "wiki/entities".to_string() },
        FileOp::Mkdir { path: "wiki/sources".to_string() },
        FileOp::Mkdir { path: "wiki/questions".to_string() },
    ];

    for (rel, content) in candidates.drain(..) {
        let exists = vault_root.exists() && vault_root.join(rel).exists();
        if !adopt || !exists {
            ops.push(FileOp::Create { path: rel.to_string(), content });
        }
    }

    let bundle = TransactionBundle {
        schema: crate::transaction::TRANSACTION_SCHEMA.to_string(),
        operation_id: format!("{}-{mode}", if adopt { "adopt-vault" } else { "init-vault" }),
        operation_type: "setup".to_string(),
        generated_at: today,
        description: format!(
            "{} Obsidian vault in '{mode}' mode",
            if adopt { "Adopt" } else { "Initialize" }
        ),
        operations: ops,
    };

    if !vault_root.exists() {
        // Preview plan without requiring directory on disk yet
        let plan_json = serde_json::json!({
            "dry_run": true,
            "vault_root": vault_root.display().to_string(),
            "operation_id": bundle.operation_id,
            "mode": mode,
            "operation_count": bundle.operations.len(),
            "operations": bundle.operations,
        });
        return Ok(plan_json);
    }

    let plan = inspect_bundle(vault_root, &bundle)?;
    if !apply {
        return Ok(serde_json::json!({
            "dry_run": true,
            "plan": plan,
        }));
    }

    let approval = approved_sha256.or(Some(plan.approval_sha256.as_str()));
    let journal = apply_bundle(vault_root, &bundle, approval)?;
    Ok(serde_json::json!({
        "dry_run": false,
        "journal": journal,
        "mode": mode,
    }))
}

/// Scans the vault in parallel using `rayon` and returns structural stats.
pub fn scan_vault(vault_root: &Path) -> Result<ScanReport> {
    assert_unaliased_directory(vault_root)?;

    let entries: Vec<_> = WalkDir::new(vault_root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            e.depth() == 0 || (name != ".git" && name != ".vault-meta" && name != ".obsidian")
        })
        .flatten()
        .filter(|e| e.file_type().is_file() && !e.file_type().is_symlink())
        .collect();

    let per_file: Vec<(String, u64, bool, bool, bool)> = entries
        .par_iter()
        .map(|entry| {
            let rel = to_posix_rel(vault_root, entry.path());
            let size = fs::metadata(entry.path()).map_or(0, |m| m.len());
            let is_md = rel.starts_with("wiki/") && rel.ends_with(".md");
            let is_inbox = rel.starts_with("inbox/") && !rel.ends_with("/.gitkeep");
            let is_raw = rel.starts_with(".raw/") && !rel.ends_with("/.manifest.json");
            let top = rel.split('/').next().unwrap_or("root").to_string();
            (top, size, is_md, is_inbox, is_raw)
        })
        .collect();

    let mut folders = BTreeMap::new();
    let mut markdown_pages = 0usize;
    let mut inbox_items = 0usize;
    let mut raw_items = 0usize;
    let mut total_bytes = 0u64;

    for (top, size, is_md, is_inbox, is_raw) in &per_file {
        *folders.entry(top.clone()).or_insert(0) += 1;
        total_bytes += *size;
        if *is_md {
            markdown_pages += 1;
        }
        if *is_inbox {
            inbox_items += 1;
        }
        if *is_raw {
            raw_items += 1;
        }
    }

    let mode = read_mode(vault_root);
    Ok(ScanReport {
        vault_root: vault_root.display().to_string(),
        mode,
        markdown_pages,
        inbox_items,
        raw_items,
        total_files: per_file.len(),
        total_bytes,
        folders,
    })
}

/// Diagnoses vault health, lock/journal state, Smart Tree availability, and MemNet mesh state.
pub fn doctor(selection: &VaultSelection) -> DoctorReport {
    let root = &selection.root;
    let meta = root.join(".vault-meta");
    let metadata_writable =
        if meta.exists() { meta.is_dir() && !meta.is_symlink() } else { root.is_dir() };

    let incomplete_journals = count_incomplete_journals(root);
    let smart_tree = probe_smart_tree();
    let memnet = probe_status();
    let mempalace = probe_mempalace();

    DoctorReport {
        ok: selection.initialized && metadata_writable && incomplete_journals == 0,
        vault_root: root.display().to_string(),
        selection_source: selection.source.clone(),
        initialized: selection.initialized,
        metadata_writable,
        incomplete_journals,
        mode: read_mode(root),
        smart_tree,
        memnet,
        mempalace,
    }
}

#[must_use]
pub fn read_mode(vault_root: &Path) -> String {
    for candidate in [".smart-vault.json", ".memvault.json", ".claude-obsidian.json"] {
        let p = vault_root.join(candidate);
        if let Ok(raw) = fs::read_to_string(p) {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&raw) {
                if let Some(m) = val.get("mode").and_then(serde_json::Value::as_str) {
                    return m.to_string();
                }
            }
        }
    }
    "standard".to_string()
}

pub fn set_mode(vault_root: &Path, mode: &str, apply: bool) -> Result<ModeReport> {
    if !VALID_MODES.contains(&mode) {
        return Err(MemVaultError::Validation(format!(
            "unsupported methodology mode '{mode}' (expected one of: {})",
            VALID_MODES.join(", ")
        )));
    }
    if !apply {
        return Ok(ModeReport {
            vault_root: vault_root.display().to_string(),
            mode: mode.to_string(),
            applied: false,
        });
    }

    let cfg_path = safe_vault_join(vault_root, Path::new(".smart-vault.json"))?;
    let mut json_obj = if cfg_path.is_file() {
        serde_json::from_str::<serde_json::Value>(&fs::read_to_string(&cfg_path)?)?
    } else {
        serde_json::json!({
            "schema": "smart-vault.workspace.v1",
            "role": "vault",
            "vault": ".",
            "source_inbox": "inbox",
            "legacy_raw": ".raw",
            "smart_tree_enabled": true,
            "memnet_enabled": true
        })
    };

    if let Some(map) = json_obj.as_object_mut() {
        map.insert("mode".to_string(), serde_json::Value::String(mode.to_string()));
    }

    crate::transaction::atomic_write_bytes(
        &cfg_path,
        format!("{}\n", serde_json::to_string_pretty(&json_obj)?).as_bytes(),
    )?;

    Ok(ModeReport {
        vault_root: vault_root.display().to_string(),
        mode: mode.to_string(),
        applied: true,
    })
}

/// Peeks the next `c-NNNNNN` address in the vault (read-only, matching `allocate-address.sh`).
pub fn peek_next_address(vault_root: &Path) -> Result<String> {
    let counter_path = safe_vault_join(vault_root, Path::new(".vault-meta/address-counter.txt"))?;
    if counter_path.is_file() {
        let raw = fs::read_to_string(&counter_path)?;
        let n: u64 = raw.trim().parse().map_err(|_| {
            MemVaultError::InvalidManifest("address-counter.txt must contain 1..999999".to_string())
        })?;
        return Ok(format!("c-{n:06}"));
    }

    let wiki = vault_root.join("wiki");
    let mut highest = 0u64;
    if wiki.is_dir() && !wiki.is_symlink() {
        for entry in WalkDir::new(&wiki).follow_links(false).into_iter().flatten() {
            if entry.file_type().is_file()
                && !entry.file_type().is_symlink()
                && entry.path().extension().and_then(|e| e.to_str()) == Some("md")
            {
                if let Ok(text) = fs::read_to_string(entry.path()) {
                    let parsed = parse_markdown(&text);
                    if let Some(addr) = parsed.frontmatter.address {
                        if let Some(num_str) = addr.strip_prefix("c-") {
                            if let Ok(num) = num_str.parse::<u64>() {
                                highest = highest.max(num);
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(format!("c-{:06}", highest + 1))
}
