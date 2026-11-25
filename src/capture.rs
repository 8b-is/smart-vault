use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::Path;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use crate::error::{MemVaultError, Result};
use crate::paths::{safe_vault_join, to_posix_rel};
use crate::transaction::{
    atomic_write_bytes, read_bounded_string, sha256_bytes, MutationLock, MAX_SINGLE_FILE_BYTES,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureAdapterInfo {
    pub id: String,
    pub maturity: String,
    pub input: String,
    pub execution: String,
    pub network: bool,
    pub extensions: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlannedCaptureItem {
    pub source_path: String,
    pub destination_raw: String,
    pub adapter: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub duplicate: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureQueueItem {
    pub id: String,
    pub source: String,
    pub kind: String,
    pub status: String, // "pending" | "claimed" | "completed" | "failed"
    pub worker: Option<String>,
    pub error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CaptureQueueState {
    pub schema: String,
    pub items: Vec<CaptureQueueItem>,
}

const QUEUE_REL: &str = ".vault-meta/capture-queue.json";

/// Built-in deterministic capture adapters matching `config/adapters.json`.
#[must_use]
pub fn builtin_adapters() -> Vec<CaptureAdapterInfo> {
    vec![
        CaptureAdapterInfo {
            id: "filesystem".to_string(),
            maturity: "implemented".to_string(),
            input: "local-file".to_string(),
            execution: "internal".to_string(),
            network: false,
            extensions: vec![
                ".md",
                ".markdown",
                ".txt",
                ".json",
                ".csv",
                ".yaml",
                ".yml",
                ".html",
                ".png",
                ".jpg",
                ".jpeg",
                ".gif",
                ".webp",
                ".pdf",
                ".epub",
                ".mp3",
                ".wav",
                ".m4a",
                ".mp4",
                ".mov",
                ".webm",
            ]
            .into_iter()
            .map(str::to_string)
            .collect(),
        },
        CaptureAdapterInfo {
            id: "url".to_string(),
            maturity: "external-runner-required".to_string(),
            input: "https-url".to_string(),
            execution: "planned-command".to_string(),
            network: true,
            extensions: vec![],
        },
        CaptureAdapterInfo {
            id: "youtube".to_string(),
            maturity: "external-runner-required".to_string(),
            input: "https-url".to_string(),
            execution: "planned-command".to_string(),
            network: true,
            extensions: vec![],
        },
        CaptureAdapterInfo {
            id: "pdf".to_string(),
            maturity: "metadata-only".to_string(),
            input: "local-file".to_string(),
            execution: "planned-command".to_string(),
            network: false,
            extensions: vec![".pdf".to_string()],
        },
        CaptureAdapterInfo {
            id: "image".to_string(),
            maturity: "metadata-only".to_string(),
            input: "local-file".to_string(),
            execution: "planned-command".to_string(),
            network: false,
            extensions: [".png", ".jpg", ".jpeg", ".gif", ".webp"]
                .into_iter()
                .map(str::to_string)
                .collect(),
        },
        CaptureAdapterInfo {
            id: "epub".to_string(),
            maturity: "metadata-only".to_string(),
            input: "local-file".to_string(),
            execution: "planned-command".to_string(),
            network: false,
            extensions: vec![".epub".to_string()],
        },
        CaptureAdapterInfo {
            id: "ocr".to_string(),
            maturity: "external-runner-required".to_string(),
            input: "local-file".to_string(),
            execution: "planned-command".to_string(),
            network: false,
            extensions: vec![],
        },
    ]
}

fn is_forbidden_v4(v4: Ipv4Addr) -> bool {
    let octets = v4.octets();
    v4.is_loopback()
        || v4.is_private()
        || v4.is_link_local()
        || v4.is_unspecified()
        || octets[0] == 0
        || v4.is_broadcast()
        || v4.is_multicast()
        || v4.is_documentation()
        || (octets[0] == 100 && (octets[1] & 0b1100_0000) == 64)
        || (octets[0] == 198 && (octets[1] & 0xfe) == 18)
        || (octets[0] == 192 && octets[1] == 0 && octets[2] == 0)
        || octets[0] >= 240
}

fn is_forbidden_v6(v6: Ipv6Addr) -> bool {
    let segs = v6.segments();
    let is_6to4_forbidden = segs[0] == 0x2002
        && is_forbidden_v4(Ipv4Addr::from((u32::from(segs[1]) << 16) | u32::from(segs[2])));
    let is_nat64_forbidden = segs[..6] == [0x0064, 0xff9b, 0, 0, 0, 0]
        && is_forbidden_v4(Ipv4Addr::from((u32::from(segs[6]) << 16) | u32::from(segs[7])));

    v6.is_loopback()
        || v6.is_unspecified()
        || v6.is_multicast()
        || (segs[0] & 0xfe00) == 0xfc00
        || (segs[0] & 0xffc0) == 0xfe80
        || (segs[0] == 0x2001 && segs[1] == 0x0db8)
        || v6.to_ipv4_mapped().is_some_and(is_forbidden_v4)
        || v6.to_ipv4().is_some_and(is_forbidden_v4)
        || is_6to4_forbidden
        || is_nat64_forbidden
}

fn parse_ipv4_component(part: &str) -> Option<u64> {
    if part.is_empty() {
        return None;
    }
    if let Some(hex) = part.strip_prefix("0x").or_else(|| part.strip_prefix("0X")) {
        if hex.is_empty() || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        return u64::from_str_radix(hex, 16).ok();
    }
    if part.starts_with('0') && part.len() > 1 {
        if !part.bytes().all(|b| matches!(b, b'0'..=b'7')) {
            return None;
        }
        return u64::from_str_radix(part, 8).ok();
    }
    if !part.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    part.parse::<u64>().ok()
}

fn parse_noncanonical_ipv4(host: &str) -> Option<Ipv4Addr> {
    let parts: Vec<&str> = host.split('.').collect();
    if parts.is_empty() || parts.len() > 4 {
        return None;
    }
    let vals: Vec<u64> =
        parts.iter().map(|p| parse_ipv4_component(p)).collect::<Option<Vec<_>>>()?;
    let addr_u32 = match vals.as_slice() {
        [a] => u32::try_from(*a).ok()?,
        [a, b] if *a <= 0xff && *b <= 0x00ff_ffff => ((*a as u32) << 24) | (*b as u32),
        [a, b, c] if *a <= 0xff && *b <= 0xff && *c <= 0xffff => {
            ((*a as u32) << 24) | ((*b as u32) << 16) | (*c as u32)
        }
        [a, b, c, d] if *a <= 0xff && *b <= 0xff && *c <= 0xff && *d <= 0xff => {
            ((*a as u32) << 24) | ((*b as u32) << 16) | ((*c as u32) << 8) | (*d as u32)
        }
        _ => return None,
    };
    Some(Ipv4Addr::from(addr_u32))
}

fn validate_port(port: &str, trimmed: &str) -> Result<()> {
    if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) || port.parse::<u16>().is_err()
    {
        return Err(MemVaultError::UnsafeUrl(format!("invalid port in URL: {trimmed}")));
    }
    Ok(())
}

/// Validates an external URL against SSRF risks (HTTPS only, blocks control/whitespace, localhost, private/mapped IPs).
pub fn validate_safe_https_url(raw_url: &str) -> Result<String> {
    if raw_url.chars().any(|c| c.is_ascii_control()) {
        return Err(MemVaultError::UnsafeUrl(
            "URL contains forbidden ASCII control characters".to_string(),
        ));
    }

    let trimmed = raw_url.trim();
    if trimmed.chars().any(char::is_whitespace) {
        return Err(MemVaultError::UnsafeUrl("URL contains embedded whitespace".to_string()));
    }

    let rest = trimmed.strip_prefix("https://").ok_or_else(|| {
        MemVaultError::UnsafeUrl(format!("only https:// URLs allowed: {trimmed}"))
    })?;

    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty()
        || authority.contains('@')
        || authority.contains('\\')
        || authority.contains('%')
    {
        return Err(MemVaultError::UnsafeUrl(format!(
            "invalid authority, backslash, percent-escape, or embedded credentials in URL: {trimmed}"
        )));
    }

    let host = if let Some(after_open) = authority.strip_prefix('[') {
        let Some((inner, after_bracket)) = after_open.split_once(']') else {
            return Err(MemVaultError::UnsafeUrl(format!(
                "malformed IPv6 bracketed authority in URL: {trimmed}"
            )));
        };
        if !after_bracket.is_empty() {
            let Some(port) = after_bracket.strip_prefix(':') else {
                return Err(MemVaultError::UnsafeUrl(format!(
                    "unexpected trailing characters after IPv6 host in URL: {trimmed}"
                )));
            };
            validate_port(port, trimmed)?;
        }
        if inner.is_empty() || inner.parse::<Ipv6Addr>().is_err() {
            return Err(MemVaultError::UnsafeUrl(format!(
                "invalid IPv6 host literal in URL: {trimmed}"
            )));
        }
        inner.to_ascii_lowercase()
    } else {
        if authority.contains('[') || authority.contains(']') {
            return Err(MemVaultError::UnsafeUrl(format!(
                "unexpected bracket in authority of URL: {trimmed}"
            )));
        }
        match authority.split_once(':') {
            Some((h, port)) => {
                validate_port(port, trimmed)?;
                h.to_ascii_lowercase()
            }
            None => authority.to_ascii_lowercase(),
        }
    };

    let host_clean = host.trim_end_matches('.');
    if host_clean.is_empty()
        || host_clean == "localhost"
        || host_clean.ends_with(".localhost")
        || host_clean.ends_with(".local")
        || host_clean.ends_with(".internal")
        || host_clean == "metadata.google.internal"
    {
        return Err(MemVaultError::UnsafeUrl(format!(
            "local or metadata host rejected: {host_clean}"
        )));
    }

    if let Ok(ip) = host_clean.parse::<IpAddr>() {
        let forbidden = match ip {
            IpAddr::V4(v4) => is_forbidden_v4(v4),
            IpAddr::V6(v6) => is_forbidden_v6(v6),
        };
        if forbidden {
            return Err(MemVaultError::UnsafeUrl(format!(
                "private or non-routable IP rejected: {ip}"
            )));
        }
    } else if let Some(v4) = parse_noncanonical_ipv4(host_clean) {
        if is_forbidden_v4(v4) {
            return Err(MemVaultError::UnsafeUrl(format!(
                "obfuscated private or non-routable IP rejected: {host_clean}"
            )));
        }
    }

    Ok(trimmed.to_string())
}

fn hash_file_streaming(path: &Path) -> Result<(String, u64)> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        total = total.saturating_add(n as u64);
    }
    Ok((hex::encode(hasher.finalize()), total))
}

/// Plans local file capture from `<vault>/inbox/` into `<vault>/.raw/` (read-only dry-run).
pub fn plan_inbox_capture(vault_root: &Path) -> Result<Vec<PlannedCaptureItem>> {
    let inbox = safe_vault_join(vault_root, Path::new("inbox"))?;
    if !inbox.is_dir() {
        return Ok(Vec::new());
    }

    let manifest_path = safe_vault_join(vault_root, Path::new(".raw/.manifest.json"))?;
    let existing_hashes: BTreeMap<String, serde_json::Value> =
        read_bounded_string(&manifest_path, MAX_SINGLE_FILE_BYTES)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|v| v.get("sources").cloned())
            .and_then(|s| serde_json::from_value(s).ok())
            .unwrap_or_default();

    let supported: Vec<&str> = vec![
        ".md",
        ".markdown",
        ".txt",
        ".json",
        ".csv",
        ".yaml",
        ".yml",
        ".html",
        ".png",
        ".jpg",
        ".jpeg",
        ".gif",
        ".webp",
        ".pdf",
        ".epub",
        ".mp3",
        ".wav",
        ".m4a",
        ".mp4",
        ".mov",
        ".webm",
    ];

    let mut planned = Vec::new();
    for entry in WalkDir::new(&inbox).follow_links(false).into_iter().flatten() {
        if !entry.file_type().is_file() || entry.file_type().is_symlink() {
            continue;
        }
        let name = entry.file_name().to_string_lossy();
        if name.starts_with('.') {
            continue;
        }
        let ext = entry
            .path()
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy().to_ascii_lowercase()))
            .unwrap_or_default();
        if !supported.contains(&ext.as_str()) {
            continue;
        }
        let (hash, size_bytes) = hash_file_streaming(entry.path())?;
        let rel = to_posix_rel(vault_root, entry.path());
        let file_name = entry.file_name().to_string_lossy();
        planned.push(PlannedCaptureItem {
            source_path: rel,
            destination_raw: format!(".raw/{}-{}", &hash[..12], file_name),
            adapter: "filesystem".to_string(),
            duplicate: existing_hashes.contains_key(&hash),
            sha256: hash,
            size_bytes,
        });
    }
    planned.sort_by(|a, b| a.source_path.cmp(&b.source_path));
    Ok(planned)
}

fn load_queue(vault_root: &Path) -> Result<CaptureQueueState> {
    let abs = safe_vault_join(vault_root, Path::new(QUEUE_REL))?;
    if !abs.is_file() {
        return Ok(CaptureQueueState {
            schema: "smart-vault.capture-queue.v1".to_string(),
            items: Vec::new(),
        });
    }
    let raw = read_bounded_string(&abs, MAX_SINGLE_FILE_BYTES)?;
    Ok(serde_json::from_str(&raw)?)
}

fn save_queue(vault_root: &Path, state: &CaptureQueueState) -> Result<()> {
    let abs = safe_vault_join(vault_root, Path::new(QUEUE_REL))?;
    atomic_write_bytes(&abs, &serde_json::to_vec_pretty(state)?)
}

pub fn queue_list(vault_root: &Path) -> Result<CaptureQueueState> {
    load_queue(vault_root)
}

pub fn queue_enqueue(vault_root: &Path, source: &str, kind: &str) -> Result<CaptureQueueItem> {
    let normalized_source = source.trim().to_ascii_lowercase();
    if kind == "url"
        || kind == "youtube"
        || normalized_source.starts_with("http://")
        || normalized_source.starts_with("https://")
    {
        validate_safe_https_url(source)?;
    }
    let _lock = MutationLock::acquire(vault_root)?;
    let mut state = load_queue(vault_root)?;
    let now = Utc::now().to_rfc3339();
    let id_seed = format!("{source}:{kind}:{}", state.items.len());
    let id = format!("cap-{}", &sha256_bytes(id_seed.as_bytes())[..12]);
    let item = CaptureQueueItem {
        id,
        source: source.to_string(),
        kind: kind.to_string(),
        status: "pending".to_string(),
        worker: None,
        error: None,
        created_at: now.clone(),
        updated_at: now,
    };
    state.items.push(item.clone());
    save_queue(vault_root, &state)?;
    Ok(item)
}

pub fn queue_claim(vault_root: &Path, worker: &str) -> Result<Option<CaptureQueueItem>> {
    let _lock = MutationLock::acquire(vault_root)?;
    let mut state = load_queue(vault_root)?;
    let mut claimed = None;
    for item in &mut state.items {
        if item.status == "pending" {
            item.status = "claimed".to_string();
            item.worker = Some(worker.to_string());
            item.updated_at = Utc::now().to_rfc3339();
            claimed = Some(item.clone());
            break;
        }
    }
    if claimed.is_some() {
        save_queue(vault_root, &state)?;
    }
    Ok(claimed)
}

pub fn queue_transition(
    vault_root: &Path,
    id: &str,
    new_status: &str,
    error: Option<&str>,
) -> Result<CaptureQueueItem> {
    let _lock = MutationLock::acquire(vault_root)?;
    let mut state = load_queue(vault_root)?;
    let item =
        state.items.iter_mut().find(|i| i.id == id).ok_or_else(|| {
            MemVaultError::Validation(format!("capture queue item not found: {id}"))
        })?;
    item.status = new_status.to_string();
    item.error = error.map(str::to_string);
    if new_status == "pending" {
        item.worker = None;
    }
    item.updated_at = Utc::now().to_rfc3339();
    let cloned = item.clone();
    save_queue(vault_root, &state)?;
    Ok(cloned)
}

pub fn queue_recover(vault_root: &Path) -> Result<Vec<String>> {
    let _lock = MutationLock::acquire(vault_root)?;
    let mut state = load_queue(vault_root)?;
    let now = Utc::now().to_rfc3339();
    let mut reset_ids = Vec::new();
    for item in &mut state.items {
        if item.status == "claimed" {
            item.status = "pending".to_string();
            item.worker = None;
            item.updated_at = now.clone();
            reset_ids.push(item.id.clone());
        }
    }
    if !reset_ids.is_empty() {
        save_queue(vault_root, &state)?;
    }
    Ok(reset_ids)
}
