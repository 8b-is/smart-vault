use std::io::{self, IsTerminal, Read};
use std::path::Path;

use serde::Serialize;

use crate::mempalace::{find_mempalace_binary, forward_hook};
use crate::paths::resolve_vault_root;
use crate::smart_tree::find_st_binary;
use crate::transaction::count_incomplete_journals;
use crate::vault::scan_vault;

#[derive(Debug, Clone, Serialize)]
pub struct HookStatus {
    pub event: String,
    pub tool: &'static str,
    pub vault_detected: bool,
    pub vault_root: Option<String>,
    pub markdown_pages: usize,
    pub inbox_items: usize,
    pub incomplete_journals: usize,
    pub smart_tree_available: bool,
    pub mempalace_available: bool,
    pub mempalace_hook_output: Option<String>,
    pub warning: Option<String>,
}

/// Emits fast, privacy-safe status JSON on `session-start` (optionally forwarding to `mempalace`).
#[must_use]
pub fn session_start(explicit_vault: Option<&Path>, with_palace: bool) -> HookStatus {
    emit_hook("session-start", explicit_vault, with_palace)
}

/// Emits fast, privacy-safe status JSON on `stop` (optionally forwarding to `mempalace`).
#[must_use]
pub fn stop(explicit_vault: Option<&Path>, with_palace: bool) -> HookStatus {
    emit_hook("stop", explicit_vault, with_palace)
}

/// Emits status JSON on `pre-compact` / `PreCompress` (optionally forwarding to `mempalace`).
#[must_use]
pub fn pre_compact(explicit_vault: Option<&Path>, with_palace: bool) -> HookStatus {
    emit_hook("pre-compact", explicit_vault, with_palace)
}

/// Emits status JSON on `session-end` (optionally forwarding to `mempalace`).
#[must_use]
pub fn session_end(explicit_vault: Option<&Path>, with_palace: bool) -> HookStatus {
    emit_hook("session-end", explicit_vault, with_palace)
}

fn read_piped_stdin_or_empty_json() -> String {
    let stdin = io::stdin();
    if stdin.is_terminal() {
        return "{}".to_string();
    }
    let mut buf = String::new();
    let _ = stdin.lock().read_to_string(&mut buf);
    if buf.trim().is_empty() {
        "{}".to_string()
    } else {
        buf
    }
}

fn map_palace_hook_name(event: &str) -> &'static str {
    match event {
        "session-start" => "session-start",
        "stop" => "stop",
        "pre-compact" => "precompact",
        "session-end" => "session-end",
        _ => "stop",
    }
}

fn emit_hook(event: &str, explicit_vault: Option<&Path>, with_palace: bool) -> HookStatus {
    let st_available = find_st_binary().is_some();
    let palace_available = find_mempalace_binary("mempalace").is_some();

    let palace_out = if with_palace && palace_available {
        let stdin_json = read_piped_stdin_or_empty_json();
        let palace_hook = map_palace_hook_name(event);
        forward_hook(palace_hook, "claude-code", &stdin_json)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    } else {
        None
    };

    let Ok(sel) = resolve_vault_root(explicit_vault, false) else {
        return HookStatus {
            event: event.to_string(),
            tool: "smart-vault",
            vault_detected: false,
            vault_root: None,
            markdown_pages: 0,
            inbox_items: 0,
            incomplete_journals: 0,
            smart_tree_available: st_available,
            mempalace_available: palace_available,
            mempalace_hook_output: palace_out,
            warning: None,
        };
    };

    let incomplete = count_incomplete_journals(&sel.root);
    let scan = scan_vault(&sel.root).ok();
    let warning = if incomplete > 0 {
        Some(format!(
            "{incomplete} incomplete transaction journal(s) detected; run `smart-vault transaction recover`"
        ))
    } else {
        None
    };

    HookStatus {
        event: event.to_string(),
        tool: "smart-vault",
        vault_detected: true,
        vault_root: Some(sel.root.display().to_string()),
        markdown_pages: scan.as_ref().map_or(0, |s| s.markdown_pages),
        inbox_items: scan.as_ref().map_or(0, |s| s.inbox_items),
        incomplete_journals: incomplete,
        smart_tree_available: st_available,
        mempalace_available: palace_available,
        mempalace_hook_output: palace_out,
        warning,
    }
}
