use std::fs;
use std::path::{Path, PathBuf};

use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::Serialize;

use crate::bm25::{benchmark_queries, build_index};
use crate::capture::{
    builtin_adapters, plan_inbox_capture, queue_claim, queue_enqueue, queue_list, queue_recover,
    queue_transition,
};
use crate::contracts::{validate_contracts, validate_package};
use crate::error::{MemVaultError, Result};
use crate::hooks::{pre_compact, session_end, session_start, stop};
use crate::lint::{lint_vault, render_markdown};
use crate::mcp::run_stdio_server;
use crate::memnet::{
    ask as memnet_ask, hybrid_query, routes as memnet_routes, share_vault, status as memnet_status,
    teach as memnet_teach,
};
use crate::mempalace::{
    daemon as palace_daemon, export_palace, mine_vault as palace_mine, search as palace_search,
    status as palace_status, wake_up as palace_wake_up,
};
use crate::multi_agent::{setup_global_m1, setup_hosts};
use crate::paths::resolve_vault_root;
use crate::signing::{sign_binary, verify_binary};
use crate::smart_tree::{run_remember, run_tree};
use crate::transaction::{apply_bundle, inspect_bundle, recover_incomplete, TransactionBundle};
use crate::vault::{doctor, init_or_adopt, peek_next_address, read_mode, scan_vault, set_mode};

#[derive(Parser, Debug)]
#[command(
    name = "smart-vault",
    version,
    about = "8b-is/smart-vault: pure-Rust #![no_std]+ASM+CSR Obsidian second brain wired into 8b-is/smart-tree, MemNet & MemPalace"
)]
pub struct Cli {
    /// Explicit path to the Obsidian vault root (or set SMART_VAULT_ROOT).
    #[arg(long, global = true, env = "SMART_VAULT_ROOT")]
    pub vault: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Inspect vault readiness, lock/journal health, Smart Tree (`st`), and MemNet mesh status.
    Doctor,

    /// Initialize a new local-first Obsidian knowledge vault (dry-run by default; pass --apply to write).
    Init(InitArgs),

    /// Adopt an existing Obsidian vault non-destructively (dry-run by default; pass --apply to write).
    Adopt(InitArgs),

    /// Scan vault markdown pages, inbox items, and raw sources in parallel.
    Scan,

    /// Lint wiki graph health (broken links, orphans, missing frontmatter, empty sections, duplicates).
    Lint(LintArgs),

    /// Build or rebuild the parallel Rust BM25+ inverted index at .vault-meta/bm25/index.json.
    Index,

    /// Query the vault via parallel BM25+ and optional MemNet MEM|8 wave cache.
    Query(QueryArgs),

    /// Evaluate offline retrieval quality against a queries JSON fixture without mutating the vault.
    Bench(BenchArgs),

    /// Render the vault hierarchy via 8b-is/smart-tree (`st --mode ...`) with pure-Rust fallback.
    Tree(TreeArgs),

    /// Sync or recall salient MEM|8 waves for the vault using `st remember`.
    Remember(RememberArgs),

    /// Interact with the MemNet mesh (`status`, `routes`, `share`, `ask`, `teach`).
    Mesh {
        #[command(subcommand)]
        action: MeshCommands,
    },

    /// Interact with MemPalace (`status`, `wake-up`, `search`, `mine`, `export`, `daemon`).
    Palace {
        #[command(subcommand)]
        action: PalaceCommands,
    },

    /// Run an agent-agnostic stdio JSON-RPC 2.0 MCP server exposing vault + Smart Tree + MemNet + MemPalace tools.
    Mcp,

    /// Inspect, apply (approval-hashed), or recover crash-safe vault transaction bundles.
    Transaction {
        #[command(subcommand)]
        action: TransactionCommands,
    },

    /// Inspect capture adapters, plan inbox capture, or manage the capture work queue.
    Capture {
        #[command(subcommand)]
        action: CaptureCommands,
    },

    /// Get or set the vault methodology mode (`standard`, `zettel`, `lyt`, `para`).
    Mode {
        #[command(subcommand)]
        action: ModeCommands,
    },

    /// Peek the next available `c-NNNNNN` page address in the vault (read-only).
    AllocateAddress,

    /// Fast agent lifecycle hooks (`session-start` | `stop` | `pre-compact` | `session-end`).
    Hook {
        /// Also forward the hook event to `mempalace hook run`.
        #[arg(long, global = true)]
        palace: bool,
        #[command(subcommand)]
        event: HookCommands,
    },

    /// Link `smart-vault` skills into Codex, Claude Code, Gemini, OpenCode, Cursor, Windsurf, or ZCode.
    SetupHost(SetupHostArgs),

    /// Globally configure Antigravity IDE + CLI (`~/.gemini`), MemPalace + `smart-vault` MCP & hooks on macOS M1.
    SetupGlobal(SetupGlobalArgs),

    /// Sign a `smart-vault` binary with macOS `codesign` and SHA-256 manifest.
    Sign(SignArgs),

    /// Verify a binary's SHA-256 digest and OS code signature strictly.
    VerifyBinary(VerifyBinaryArgs),

    /// Validate repository product & capability contracts.
    Contracts(RepoRootArgs),

    /// Validate portable package metadata, hooks, and skills frontmatter.
    Package {
        #[command(subcommand)]
        action: PackageCommands,
    },
}

#[derive(Args, Debug)]
pub struct InitArgs {
    #[arg(long, default_value = "standard")]
    pub mode: String,
    #[arg(long)]
    pub apply: bool,
    #[arg(long)]
    pub approved_plan_sha256: Option<String>,
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum LintFormat {
    Json,
    Markdown,
}

#[derive(Args, Debug)]
pub struct LintArgs {
    #[arg(long, value_enum, default_value_t = LintFormat::Json)]
    pub format: LintFormat,
}

#[derive(Args, Debug)]
pub struct QueryArgs {
    pub query: String,
    #[arg(long, default_value_t = 8)]
    pub top: usize,
    /// Also check MemNet wave cache (`st memnet ask`) before/alongside vault search.
    #[arg(long)]
    pub mesh: bool,
    /// Also query MemPalace verbatim drawers (`mempalace search`).
    #[arg(long)]
    pub palace: bool,
    #[arg(long)]
    pub no_auto_build: bool,
}

#[derive(Args, Debug)]
pub struct BenchArgs {
    #[arg(long)]
    pub queries: PathBuf,
    #[arg(long, default_value_t = 5)]
    pub top: usize,
}

#[derive(Args, Debug)]
pub struct TreeArgs {
    #[arg(long, default_value = "ai")]
    pub mode: String,
    #[arg(long)]
    pub depth: Option<usize>,
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct RememberArgs {
    #[arg(long, default_value = "expert")]
    pub level: String,
    #[arg(long)]
    pub check: bool,
    #[arg(long)]
    pub force: bool,
    #[arg(short, long)]
    pub query: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum MeshCommands {
    /// Show MemNet node address, tier, and active routes.
    Status,
    /// List live MemNet routes and RTT/salience telemetry.
    Routes,
    /// Publish the vault directory onto MemNet for local peers.
    Share {
        #[arg(long, default_value_t = 4096)]
        quota_mb: u64,
    },
    /// Query MemNet MEM|8 wave cache.
    Ask { question: String },
    /// Anchor a Q&A wave packet into MemNet.
    Teach { question: String, answer: String },
}

#[derive(Subcommand, Debug)]
pub enum PalaceCommands {
    /// Inspect MemPalace health (`repair-status` by default, or full ChromaDB `status` with `--full`).
    Status {
        #[arg(long)]
        full: bool,
    },
    /// Load L0 + L1 MemPalace wake-up context.
    WakeUp {
        #[arg(long)]
        wing: Option<String>,
    },
    /// Search MemPalace verbatim drawers semantically.
    Search {
        query: String,
        #[arg(long)]
        wing: Option<String>,
        #[arg(long)]
        room: Option<String>,
        #[arg(long)]
        results: Option<usize>,
    },
    /// Mine the vault's `wiki/` directory into MemPalace drawers.
    Mine {
        #[arg(long)]
        wing: Option<String>,
        #[arg(long)]
        dry_run: bool,
    },
    /// Export MemPalace drawers into a Markdown folder.
    Export { out_dir: PathBuf },
    /// Manage the warm MemPalace background daemon (`status`, `start`, `stop`, `jobs`).
    Daemon {
        #[arg(default_value = "status")]
        action: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum TransactionCommands {
    /// Compute canonical SHA-256 plan hash for a transaction bundle JSON file.
    Inspect { bundle: PathBuf },
    /// Apply a reviewed transaction bundle JSON file atomically under lock.
    Apply {
        bundle: PathBuf,
        #[arg(long)]
        approved_plan_sha256: Option<String>,
    },
    /// Recover incomplete `in_progress` transaction journals (dry-run unless --apply).
    Recover {
        #[arg(long)]
        apply: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum CaptureCommands {
    /// List built-in and external capture adapter definitions.
    Adapters,
    /// Plan local filesystem capture from inbox/ to .raw/ (read-only).
    Plan,
    /// Manage the persistent capture queue at .vault-meta/capture-queue.json.
    Queue {
        #[command(subcommand)]
        action: CaptureQueueCommands,
    },
}

#[derive(Subcommand, Debug)]
pub enum CaptureQueueCommands {
    List,
    Enqueue {
        source: String,
        #[arg(long, default_value = "filesystem")]
        kind: String,
    },
    Claim {
        #[arg(long, default_value = "smart-vault-worker")]
        worker: String,
    },
    Complete {
        id: String,
    },
    Fail {
        id: String,
        #[arg(long, default_value = "unspecified failure")]
        error: String,
    },
    Resume {
        id: String,
    },
    Recover,
}

#[derive(Subcommand, Debug)]
pub enum ModeCommands {
    Get,
    Set {
        mode: String,
        #[arg(long)]
        apply: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum HookCommands {
    SessionStart,
    Stop,
    PreCompact,
    SessionEnd,
}

#[derive(Args, Debug)]
pub struct SetupHostArgs {
    #[arg(long = "host", required = true)]
    pub hosts: Vec<String>,
    #[arg(long)]
    pub workspace: Option<PathBuf>,
    #[arg(long)]
    pub repo_root: Option<PathBuf>,
    #[arg(long)]
    pub apply: bool,
}

#[derive(Args, Debug)]
pub struct SetupGlobalArgs {
    /// Source `smart-vault` binary to install into `~/.local/bin/smart-vault` (defaults to current executable).
    #[arg(long)]
    pub binary: Option<PathBuf>,
    /// Use `mempalace-light-mcp` (3 consolidated tools) instead of full `mempalace-mcp`.
    #[arg(long)]
    pub light_mcp: bool,
    /// Product repo root containing `skills/`.
    #[arg(long)]
    pub repo_root: Option<PathBuf>,
    /// Apply changes globally (defaults to dry-run preview).
    #[arg(long)]
    pub apply: bool,
}

#[derive(Args, Debug)]
pub struct SignArgs {
    /// Path to binary executable (defaults to current running executable).
    pub binary: Option<PathBuf>,
    /// Optional Apple Developer ID / certificate identity (defaults to ad-hoc `-`).
    #[arg(long)]
    pub identity: Option<String>,
    /// Write `<binary>.sha256` sidecar file alongside the signed binary.
    #[arg(long)]
    pub write_sha256: bool,
}

#[derive(Args, Debug)]
pub struct VerifyBinaryArgs {
    /// Path to binary executable (defaults to current running executable).
    pub binary: Option<PathBuf>,
    /// Optional expected SHA-256 hex digest to verify in constant time.
    #[arg(long)]
    pub sha256: Option<String>,
}

#[derive(Args, Debug)]
pub struct RepoRootArgs {
    #[arg(long, default_value = ".")]
    pub repo_root: PathBuf,
}

#[derive(Subcommand, Debug)]
pub enum PackageCommands {
    Validate(RepoRootArgs),
}

fn emit_json<T: Serialize>(value: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

/// Executes a parsed `Cli` command.
pub fn dispatch(cli: Cli) -> Result<()> {
    let vault_ref = cli.vault.as_deref();

    match cli.command {
        Commands::Doctor => {
            let sel = resolve_vault_root(vault_ref, true)?;
            emit_json(&doctor(&sel))
        }
        Commands::Init(args) => {
            let sel = resolve_vault_root(vault_ref, true)?;
            let out = init_or_adopt(
                &sel.root,
                &args.mode,
                false,
                args.apply,
                args.approved_plan_sha256.as_deref(),
            )?;
            emit_json(&out)
        }
        Commands::Adopt(args) => {
            let sel = resolve_vault_root(vault_ref, true)?;
            let out = init_or_adopt(
                &sel.root,
                &args.mode,
                true,
                args.apply,
                args.approved_plan_sha256.as_deref(),
            )?;
            emit_json(&out)
        }
        Commands::Scan => {
            let sel = resolve_vault_root(vault_ref, false)?;
            emit_json(&scan_vault(&sel.root)?)
        }
        Commands::Lint(args) => {
            let sel = resolve_vault_root(vault_ref, false)?;
            let report = lint_vault(&sel.root)?;
            match args.format {
                LintFormat::Json => emit_json(&report),
                LintFormat::Markdown => {
                    print!("{}", render_markdown(&report));
                    Ok(())
                }
            }
        }
        Commands::Index => {
            let sel = resolve_vault_root(vault_ref, false)?;
            emit_json(&build_index(&sel.root)?)
        }
        Commands::Query(args) => {
            let sel = resolve_vault_root(vault_ref, false)?;
            let resp = hybrid_query(
                &sel.root,
                &args.query,
                args.top,
                args.mesh,
                args.palace,
                !args.no_auto_build,
            )?;
            emit_json(&resp)
        }
        Commands::Bench(args) => {
            let sel = resolve_vault_root(vault_ref, false)?;
            emit_json(&benchmark_queries(&sel.root, &args.queries, args.top)?)
        }
        Commands::Tree(args) => {
            let sel = resolve_vault_root(vault_ref, false)?;
            let out = run_tree(&sel.root, &args.mode, args.depth)?;
            if args.json {
                emit_json(&out)
            } else {
                print!("{}", out.output);
                Ok(())
            }
        }
        Commands::Remember(args) => {
            let sel = resolve_vault_root(vault_ref, false)?;
            let out = run_remember(
                &sel.root,
                &args.level,
                args.check,
                args.query.as_deref(),
                args.force,
            )?;
            emit_json(&out)
        }
        Commands::Mesh { action } => match action {
            MeshCommands::Status => emit_json(&memnet_status()?),
            MeshCommands::Routes => emit_json(&memnet_routes()?),
            MeshCommands::Share { quota_mb } => {
                let sel = resolve_vault_root(vault_ref, false)?;
                emit_json(&share_vault(&sel.root, quota_mb)?)
            }
            MeshCommands::Ask { question } => emit_json(&memnet_ask(&question, None)?),
            MeshCommands::Teach { question, answer } => {
                emit_json(&memnet_teach(&question, &answer)?)
            }
        },
        Commands::Palace { action } => match action {
            PalaceCommands::Status { full } => emit_json(&palace_status(full)?),
            PalaceCommands::WakeUp { wing } => emit_json(&palace_wake_up(wing.as_deref())?),
            PalaceCommands::Search { query, wing, room, results } => {
                emit_json(&palace_search(&query, wing.as_deref(), room.as_deref(), results)?)
            }
            PalaceCommands::Mine { wing, dry_run } => {
                let sel = resolve_vault_root(vault_ref, false)?;
                emit_json(&palace_mine(&sel.root, wing.as_deref(), dry_run)?)
            }
            PalaceCommands::Export { out_dir } => emit_json(&export_palace(&out_dir)?),
            PalaceCommands::Daemon { action } => emit_json(&palace_daemon(&action)?),
        },
        Commands::Mcp => {
            run_stdio_server(cli.vault)?;
            Ok(())
        }
        Commands::Transaction { action } => {
            let sel = resolve_vault_root(vault_ref, false)?;
            match action {
                TransactionCommands::Inspect { bundle } => {
                    let parsed = load_bundle(&bundle)?;
                    emit_json(&inspect_bundle(&sel.root, &parsed)?)
                }
                TransactionCommands::Apply { bundle, approved_plan_sha256 } => {
                    let parsed = load_bundle(&bundle)?;
                    emit_json(&apply_bundle(&sel.root, &parsed, approved_plan_sha256.as_deref())?)
                }
                TransactionCommands::Recover { apply } => {
                    emit_json(&recover_incomplete(&sel.root, apply)?)
                }
            }
        }
        Commands::Capture { action } => match action {
            CaptureCommands::Adapters => emit_json(&builtin_adapters()),
            CaptureCommands::Plan => {
                let sel = resolve_vault_root(vault_ref, false)?;
                emit_json(&plan_inbox_capture(&sel.root)?)
            }
            CaptureCommands::Queue { action } => {
                let sel = resolve_vault_root(vault_ref, false)?;
                match action {
                    CaptureQueueCommands::List => emit_json(&queue_list(&sel.root)?),
                    CaptureQueueCommands::Enqueue { source, kind } => {
                        emit_json(&queue_enqueue(&sel.root, &source, &kind)?)
                    }
                    CaptureQueueCommands::Claim { worker } => {
                        emit_json(&queue_claim(&sel.root, &worker)?)
                    }
                    CaptureQueueCommands::Complete { id } => {
                        emit_json(&queue_transition(&sel.root, &id, "completed", None)?)
                    }
                    CaptureQueueCommands::Fail { id, error } => {
                        emit_json(&queue_transition(&sel.root, &id, "failed", Some(&error))?)
                    }
                    CaptureQueueCommands::Resume { id } => {
                        emit_json(&queue_transition(&sel.root, &id, "pending", None)?)
                    }
                    CaptureQueueCommands::Recover => emit_json(&queue_recover(&sel.root)?),
                }
            }
        },
        Commands::Mode { action } => {
            let sel = resolve_vault_root(vault_ref, false)?;
            match action {
                ModeCommands::Get => emit_json(&serde_json::json!({
                    "vault_root": sel.root.display().to_string(),
                    "mode": read_mode(&sel.root),
                })),
                ModeCommands::Set { mode, apply } => emit_json(&set_mode(&sel.root, &mode, apply)?),
            }
        }
        Commands::AllocateAddress => {
            let sel = resolve_vault_root(vault_ref, false)?;
            let next = peek_next_address(&sel.root)?;
            emit_json(&serde_json::json!({
                "vault_root": sel.root.display().to_string(),
                "next_address": next,
            }))
        }
        Commands::Hook { palace, event } => match event {
            HookCommands::SessionStart => emit_json(&session_start(vault_ref, palace)),
            HookCommands::Stop => emit_json(&stop(vault_ref, palace)),
            HookCommands::PreCompact => emit_json(&pre_compact(vault_ref, palace)),
            HookCommands::SessionEnd => emit_json(&session_end(vault_ref, palace)),
        },
        Commands::SetupHost(args) => {
            let root = args
                .repo_root
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
            emit_json(&setup_hosts(&root, &args.hosts, args.workspace.as_deref(), args.apply)?)
        }
        Commands::SetupGlobal(args) => {
            let root = args
                .repo_root
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
            emit_json(&setup_global_m1(
                &root,
                args.binary.as_deref(),
                None,
                args.light_mcp,
                args.apply,
            )?)
        }
        Commands::Sign(args) => {
            let bin = match args.binary {
                Some(p) => p,
                None => std::env::current_exe()?,
            };
            emit_json(&sign_binary(&bin, args.identity.as_deref(), args.write_sha256)?)
        }
        Commands::VerifyBinary(args) => {
            let bin = match args.binary {
                Some(p) => p,
                None => std::env::current_exe()?,
            };
            emit_json(&verify_binary(&bin, args.sha256.as_deref())?)
        }
        Commands::Contracts(args) => {
            let report = validate_contracts(&args.repo_root)?;
            emit_json(&report)?;
            if !report.ok {
                return Err(MemVaultError::Contract(format!(
                    "{} contract error(s) detected",
                    report.findings.len()
                )));
            }
            Ok(())
        }
        Commands::Package { action } => match action {
            PackageCommands::Validate(args) => {
                let report = validate_package(&args.repo_root)?;
                emit_json(&report)?;
                if !report.ok {
                    return Err(MemVaultError::Contract(format!(
                        "{} package validation finding(s) detected",
                        report.findings.len()
                    )));
                }
                Ok(())
            }
        },
    }
}

fn load_bundle(path: &Path) -> Result<TransactionBundle> {
    let raw = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&raw)?)
}
