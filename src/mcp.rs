use std::io::{self, BufRead, Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::bm25::build_index;
use crate::lint::lint_vault;
use crate::memnet::{
    ask as memnet_ask, hybrid_query, share_vault, status as memnet_status, teach as memnet_teach,
};
use crate::mempalace::{
    mine_vault as palace_mine, probe_mempalace, search as palace_search, status as palace_status,
    wake_up as palace_wake_up,
};
use crate::paths::resolve_vault_root;
use crate::smart_tree::{run_remember, run_tree};
use crate::vault::{doctor, scan_vault};

/// Maximum incoming JSON-RPC request line size (4 MiB) to prevent OOM on malformed/unterminated streams.
pub const MAX_RPC_LINE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Deserialize)]
struct RpcRequest {
    jsonrpc: Option<String>,
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Serialize)]
struct RpcResponse {
    jsonrpc: &'static str,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<RpcError>,
}

#[derive(Debug, Serialize)]
struct RpcError {
    code: i64,
    message: String,
}

enum BoundedLine {
    Eof,
    Line(Vec<u8>),
    TooLong,
}

fn drain_until_newline<R: BufRead>(reader: &mut R) -> io::Result<()> {
    loop {
        let chunk = reader.fill_buf()?;
        if chunk.is_empty() {
            return Ok(());
        }
        if let Some(pos) = chunk.iter().position(|&b| b == b'\n') {
            reader.consume(pos + 1);
            return Ok(());
        }
        let len = chunk.len();
        reader.consume(len);
    }
}

fn read_bounded_line<R: BufRead>(reader: &mut R, max_bytes: usize) -> io::Result<BoundedLine> {
    let mut buf = Vec::new();
    let limit = (max_bytes as u64).saturating_add(1);
    let n = reader.by_ref().take(limit).read_until(b'\n', &mut buf)?;
    if n == 0 {
        return Ok(BoundedLine::Eof);
    }
    if buf.len() > max_bytes {
        if !buf.ends_with(b"\n") {
            drain_until_newline(reader)?;
        }
        return Ok(BoundedLine::TooLong);
    }
    Ok(BoundedLine::Line(buf))
}

/// Runs a newline-delimited JSON-RPC 2.0 MCP server on stdin/stdout.
/// All diagnostics go exclusively to stderr so stdout stays strictly JSON-RPC.
pub fn run_stdio_server(default_vault: Option<PathBuf>) -> io::Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut reader = stdin.lock();
    let mut out = stdout.lock();
    run_stdio_server_with_io(&mut reader, &mut out, default_vault.as_deref(), MAX_RPC_LINE_BYTES)
}

fn run_stdio_server_with_io<R: BufRead, W: Write>(
    reader: &mut R,
    out: &mut W,
    default_vault: Option<&Path>,
    max_line_bytes: usize,
) -> io::Result<()> {
    loop {
        let line_bytes = match read_bounded_line(reader, max_line_bytes)? {
            BoundedLine::Eof => break,
            BoundedLine::TooLong => {
                let resp = RpcResponse {
                    jsonrpc: "2.0",
                    id: Value::Null,
                    result: None,
                    error: Some(RpcError {
                        code: -32600,
                        message: format!(
                            "request exceeds maximum line length of {max_line_bytes} bytes"
                        ),
                    }),
                };
                write_json_line(out, &resp)?;
                continue;
            }
            BoundedLine::Line(bytes) => bytes,
        };

        let line = match std::str::from_utf8(&line_bytes) {
            Ok(s) => s,
            Err(err) => {
                let resp = RpcResponse {
                    jsonrpc: "2.0",
                    id: Value::Null,
                    result: None,
                    error: Some(RpcError {
                        code: -32700,
                        message: format!("invalid UTF-8 in request: {err}"),
                    }),
                };
                write_json_line(out, &resp)?;
                continue;
            }
        };

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let req: RpcRequest = match serde_json::from_str(trimmed) {
            Ok(r) => r,
            Err(err) => {
                let resp = RpcResponse {
                    jsonrpc: "2.0",
                    id: Value::Null,
                    result: None,
                    error: Some(RpcError { code: -32700, message: format!("parse error: {err}") }),
                };
                write_json_line(out, &resp)?;
                continue;
            }
        };

        if let Some(ref version) = req.jsonrpc {
            if version != "2.0" {
                if let Some(id) = req.id.clone() {
                    let resp = RpcResponse {
                        jsonrpc: "2.0",
                        id,
                        result: None,
                        error: Some(RpcError {
                            code: -32600,
                            message: format!(
                                "unsupported jsonrpc version '{version}', expected '2.0'"
                            ),
                        }),
                    };
                    write_json_line(out, &resp)?;
                }
                continue;
            }
        }

        // Notifications have no id and expect no response
        let Some(id) = req.id.clone() else {
            continue;
        };

        let resp = match req.method.as_str() {
            "initialize" => RpcResponse {
                jsonrpc: "2.0",
                id,
                result: Some(json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {
                        "tools": { "listChanged": false }
                    },
                    "serverInfo": {
                        "name": "smart-vault",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                })),
                error: None,
            },
            "ping" => RpcResponse { jsonrpc: "2.0", id, result: Some(json!({})), error: None },
            "tools/list" => RpcResponse {
                jsonrpc: "2.0",
                id,
                result: Some(json!({ "tools": tool_definitions() })),
                error: None,
            },
            "tools/call" => handle_tool_call(id, &req.params, default_vault),
            other => RpcResponse {
                jsonrpc: "2.0",
                id,
                result: None,
                error: Some(RpcError {
                    code: -32601,
                    message: format!("method not found: {other}"),
                }),
            },
        };

        write_json_line(out, &resp)?;
    }

    Ok(())
}

fn write_json_line<W: Write, T: Serialize>(out: &mut W, value: &T) -> io::Result<()> {
    serde_json::to_writer(&mut *out, value)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    out.write_all(b"\n")?;
    out.flush()
}

fn handle_tool_call(
    id: Value,
    params: &Value,
    default_vault: Option<&std::path::Path>,
) -> RpcResponse {
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return RpcResponse {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(RpcError {
                code: -32602,
                message: "missing tools/call params.name".to_string(),
            }),
        };
    };
    let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));

    let vault_override = args.get("vault").and_then(Value::as_str).map(PathBuf::from);
    let vault_arg = vault_override.as_deref().or(default_vault);

    let outcome: std::result::Result<Value, String> = match name {
        "vault_doctor" => resolve_vault_root(vault_arg, true)
            .map(|sel| serde_json::to_value(doctor(&sel)).unwrap_or(Value::Null))
            .map_err(|e| format!("{}: {e}", e.code())),
        "vault_scan" => resolve_vault_root(vault_arg, false)
            .and_then(|sel| scan_vault(&sel.root))
            .map(|rep| serde_json::to_value(rep).unwrap_or(Value::Null))
            .map_err(|e| format!("{}: {e}", e.code())),
        "vault_tree" => {
            let mode = args.get("mode").and_then(Value::as_str).unwrap_or("ai");
            let depth = args
                .get("depth")
                .and_then(Value::as_u64)
                .map(|d| usize::try_from(d).unwrap_or(1024).min(1024));
            resolve_vault_root(vault_arg, false)
                .and_then(|sel| run_tree(&sel.root, mode, depth))
                .map(|rep| serde_json::to_value(rep).unwrap_or(Value::Null))
                .map_err(|e| format!("{}: {e}", e.code()))
        }
        "vault_lint" => resolve_vault_root(vault_arg, false)
            .and_then(|sel| lint_vault(&sel.root))
            .map(|rep| serde_json::to_value(rep).unwrap_or(Value::Null))
            .map_err(|e| format!("{}: {e}", e.code())),
        "vault_index" => resolve_vault_root(vault_arg, false)
            .and_then(|sel| build_index(&sel.root))
            .map(|rep| serde_json::to_value(rep).unwrap_or(Value::Null))
            .map_err(|e| format!("{}: {e}", e.code())),
        "vault_query" => {
            let query = args.get("query").and_then(Value::as_str).unwrap_or("");
            let top_k = args
                .get("top_k")
                .and_then(Value::as_u64)
                .map_or(8, |n| usize::try_from(n).unwrap_or(1000).min(1000));
            let use_mesh = args.get("use_mesh").and_then(Value::as_bool).unwrap_or(false);
            let use_palace = args.get("use_palace").and_then(Value::as_bool).unwrap_or(false);
            resolve_vault_root(vault_arg, false)
                .and_then(|sel| hybrid_query(&sel.root, query, top_k, use_mesh, use_palace, true))
                .map(|rep| serde_json::to_value(rep).unwrap_or(Value::Null))
                .map_err(|e| format!("{}: {e}", e.code()))
        }
        "vault_remember" => {
            let level = args.get("level").and_then(Value::as_str).unwrap_or("expert");
            let check = args.get("check").and_then(Value::as_bool).unwrap_or(false);
            let force = args.get("force").and_then(Value::as_bool).unwrap_or(false);
            let query = args.get("query").and_then(Value::as_str);
            resolve_vault_root(vault_arg, false)
                .and_then(|sel| run_remember(&sel.root, level, check, query, force))
                .map(|rep| serde_json::to_value(rep).unwrap_or(Value::Null))
                .map_err(|e| format!("{}: {e}", e.code()))
        }
        "mesh_status" => memnet_status().map_err(|e| format!("{}: {e}", e.code())),
        "mesh_ask" => {
            let question = args.get("question").and_then(Value::as_str).unwrap_or("");
            memnet_ask(question, None).map_err(|e| format!("{}: {e}", e.code()))
        }
        "mesh_teach" => {
            let question = args.get("question").and_then(Value::as_str).unwrap_or("");
            let answer = args.get("answer").and_then(Value::as_str).unwrap_or("");
            memnet_teach(question, answer).map_err(|e| format!("{}: {e}", e.code()))
        }
        "mesh_share" => {
            let quota_mb = args.get("quota_mb").and_then(Value::as_u64).unwrap_or(4096);
            resolve_vault_root(vault_arg, false)
                .and_then(|sel| share_vault(&sel.root, quota_mb))
                .map_err(|e| format!("{}: {e}", e.code()))
        }
        "palace_status" => {
            let full = args.get("full").and_then(Value::as_bool).unwrap_or(false);
            let info = probe_mempalace();
            let cli_out = if info.available { palace_status(full).ok() } else { None };
            Ok(json!({ "info": info, "status": cli_out }))
        }
        "palace_wake_up" => {
            let wing = args.get("wing").and_then(Value::as_str);
            palace_wake_up(wing)
                .map(|out| serde_json::to_value(out).unwrap_or(Value::Null))
                .map_err(|e| format!("{}: {e}", e.code()))
        }
        "palace_search" => {
            let query = args.get("query").and_then(Value::as_str).unwrap_or("");
            let wing = args.get("wing").and_then(Value::as_str);
            let room = args.get("room").and_then(Value::as_str);
            let results = args
                .get("results")
                .and_then(Value::as_u64)
                .map(|n| usize::try_from(n).unwrap_or(1000).min(1000));
            palace_search(query, wing, room, results)
                .map(|out| serde_json::to_value(out).unwrap_or(Value::Null))
                .map_err(|e| format!("{}: {e}", e.code()))
        }
        "palace_mine" => {
            let wing = args.get("wing").and_then(Value::as_str);
            let dry_run = args.get("dry_run").and_then(Value::as_bool).unwrap_or(false);
            resolve_vault_root(vault_arg, false)
                .and_then(|sel| palace_mine(&sel.root, wing, dry_run))
                .map(|out| serde_json::to_value(out).unwrap_or(Value::Null))
                .map_err(|e| format!("{}: {e}", e.code()))
        }
        other => Err(format!("unknown tool: {other}")),
    };

    match outcome {
        Ok(val) => {
            let text = serde_json::to_string_pretty(&val).unwrap_or_else(|_| "{}".to_string());
            RpcResponse {
                jsonrpc: "2.0",
                id,
                result: Some(json!({
                    "content": [{ "type": "text", "text": text }],
                    "structuredContent": val,
                    "isError": false
                })),
                error: None,
            }
        }
        Err(msg) => RpcResponse {
            jsonrpc: "2.0",
            id,
            result: Some(json!({
                "content": [{ "type": "text", "text": msg }],
                "isError": true
            })),
            error: None,
        },
    }
}

fn tool_definitions() -> Value {
    json!([
        {
            "name": "vault_doctor",
            "description": "Inspect Obsidian vault readiness, lock/journal state, 8b-is/smart-tree availability, and MemNet mesh reachability.",
            "inputSchema": {
                "type": "object",
                "properties": { "vault": { "type": "string" } }
            }
        },
        {
            "name": "vault_scan",
            "description": "Scan Obsidian vault pages, inbox items, raw sources, and directory layout.",
            "inputSchema": {
                "type": "object",
                "properties": { "vault": { "type": "string" } }
            }
        },
        {
            "name": "vault_tree",
            "description": "Render vault structure via 8b-is/smart-tree (`st --mode ai|classic|quantum|json`) with pure-Rust fallback.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "vault": { "type": "string" },
                    "mode": { "type": "string", "default": "ai" },
                    "depth": { "type": "integer" }
                }
            }
        },
        {
            "name": "vault_lint",
            "description": "Run parallel graph & structural lint checks across wiki/**/*.md (dangling links, orphans, missing frontmatter, empty sections, duplicates).",
            "inputSchema": {
                "type": "object",
                "properties": { "vault": { "type": "string" } }
            }
        },
        {
            "name": "vault_index",
            "description": "Build or rebuild the parallel Rust BM25+ inverted index at .vault-meta/bm25/index.json.",
            "inputSchema": {
                "type": "object",
                "properties": { "vault": { "type": "string" } }
            }
        },
        {
            "name": "vault_query",
            "description": "Hybrid retrieval: searches local Obsidian vault via parallel BM25+ and optionally checks MemNet wave cache and MemPalace drawers.",
            "inputSchema": {
                "type": "object",
                "required": ["query"],
                "properties": {
                    "vault": { "type": "string" },
                    "query": { "type": "string" },
                    "top_k": { "type": "integer", "default": 8 },
                    "use_mesh": { "type": "boolean", "default": false },
                    "use_palace": { "type": "boolean", "default": false }
                }
            }
        },
        {
            "name": "vault_remember",
            "description": "Persist or recall salient MEM|8 waves for the Obsidian vault using `st remember`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "vault": { "type": "string" },
                    "level": { "type": "string", "default": "expert" },
                    "check": { "type": "boolean", "default": false },
                    "force": { "type": "boolean", "default": false },
                    "query": { "type": "string" }
                }
            }
        },
        {
            "name": "mesh_status",
            "description": "Inspect MemNet local address, tier, and live mesh routes.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "mesh_ask",
            "description": "Query MemNet MEM|8 wave cache for a cached Q&A hit.",
            "inputSchema": {
                "type": "object",
                "required": ["question"],
                "properties": { "question": { "type": "string" } }
            }
        },
        {
            "name": "mesh_teach",
            "description": "Anchor a Q&A packet into MemNet MEM|8 wave memory.",
            "inputSchema": {
                "type": "object",
                "required": ["question", "answer"],
                "properties": {
                    "question": { "type": "string" },
                    "answer": { "type": "string" }
                }
            }
        },
        {
            "name": "mesh_share",
            "description": "Publish the Obsidian vault folder on MemNet for nearby peers.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "vault": { "type": "string" },
                    "quota_mb": { "type": "integer", "default": 4096 }
                }
            }
        },
        {
            "name": "palace_status",
            "description": "Inspect local MemPalace binaries, palace store path, and repair-status counts.",
            "inputSchema": {
                "type": "object",
                "properties": { "full": { "type": "boolean", "default": false } }
            }
        },
        {
            "name": "palace_wake_up",
            "description": "Load MemPalace L0+L1 wake-up context, optionally scoped to a wing.",
            "inputSchema": {
                "type": "object",
                "properties": { "wing": { "type": "string" } }
            }
        },
        {
            "name": "palace_search",
            "description": "Search verbatim MemPalace drawers semantically across wings and rooms.",
            "inputSchema": {
                "type": "object",
                "required": ["query"],
                "properties": {
                    "query": { "type": "string" },
                    "wing": { "type": "string" },
                    "room": { "type": "string" },
                    "results": { "type": "integer", "default": 5 }
                }
            }
        },
        {
            "name": "palace_mine",
            "description": "Mine the Obsidian vault wiki/ directory into MemPalace drawers.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "vault": { "type": "string" },
                    "wing": { "type": "string" },
                    "dry_run": { "type": "boolean", "default": false }
                }
            }
        }
    ])
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    #[test]
    fn rejects_oversized_and_unterminated_lines_without_losing_framing() {
        let oversized = "x".repeat(256);
        let input = format!("{oversized}\n{{\"jsonrpc\":\"2.0\",\"id\":7,\"method\":\"ping\"}}\n");
        let mut reader = Cursor::new(input.into_bytes());
        let mut out = Vec::new();
        run_stdio_server_with_io(&mut reader, &mut out, None, 128).expect("server ok");

        let lines: Vec<Value> = String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["error"]["code"], -32600);
        assert_eq!(lines[1]["id"], 7);
        assert_eq!(lines[1]["result"], json!({}));

        // Unterminated oversized stream still terminates cleanly with a single error
        let unterminated = vec![b'a'; 300];
        let mut reader2 = Cursor::new(unterminated);
        let mut out2 = Vec::new();
        run_stdio_server_with_io(&mut reader2, &mut out2, None, 128).expect("server ok");
        let lines2: Vec<Value> = String::from_utf8(out2)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines2.len(), 1);
        assert_eq!(lines2[0]["error"]["code"], -32600);
    }

    #[test]
    fn rejects_unsupported_jsonrpc_version() {
        let input = "{\"jsonrpc\":\"1.0\",\"id\":9,\"method\":\"ping\"}\n";
        let mut reader = Cursor::new(input.as_bytes());
        let mut out = Vec::new();
        run_stdio_server_with_io(&mut reader, &mut out, None, MAX_RPC_LINE_BYTES).expect("ok");
        let resp: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(resp["id"], 9);
        assert_eq!(resp["error"]["code"], -32600);
    }
}
