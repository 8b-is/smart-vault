# smart-vault: Agent Instructions

`smart-vault` (`8b-is/smart-vault`, evolved from `claude-obsidian`, **made with `<3` from `8b.is-crew`**) is a **100% pure-Rust, agent-agnostic** local-first Obsidian knowledge-graph engine built on a zero-allocation `#![no_std]` core (`crates/smart-vault-core` with `aarch64` assembly/NEON SIMD, CSR graph analytics, iterative Tarjan SCC, PageRank, and IEEE-754 BM25+ math) wired directly into **`8b-is/smart-tree` (`st`)**, **`MemNet`**, and **`MemPalace`**.

It exposes both a native signed CLI (`smart-vault`, alias `sv`) with `mimalloc` and an agent-agnostic stdio JSON-RPC 2.0 MCP server (`smart-vault mcp`) for Antigravity IDE & CLI, Codex, Claude Code, Gemini, OpenCode, Cursor, Windsurf, ZCode, and any MCP/Agent-Skills host.

## Product and vault boundaries

- This repository is the product source (`crates/`, `src/`, `skills/`, `templates/`). It is **not** a user vault.
- A user vault is the directory containing `.smart-vault.json` (or legacy `.memvault.json` / `.claude-obsidian.json`), `wiki/`, and `.raw/`.
- Resolve a vault in this order: explicit `--vault PATH`, `SMART_VAULT_ROOT`, `MEMVAULT_ROOT`, `CLAUDE_OBSIDIAN_VAULT`, or walking up from `CWD`. Fail closed when no vault is selected.
- Raw payloads under `.raw/` are create-only. Destructive repairs and remote network egress require explicit user consent.

## Bootstrap

1. Read this file and load the matching skill under `skills/<name>/SKILL.md`.
2. Resolve the user vault with `smart-vault doctor --vault <path>` (or MCP tool `vault_doctor`).
3. For a new vault, preview `smart-vault init --vault <path>` first; pass `--apply` only after review. Use `smart-vault adopt` for an existing Obsidian vault.
4. For visual structure and code/note context, call `smart-vault tree --mode ai` (powered by `8b-is/smart-tree` `st` with pure-Rust fallback).
5. For persistent `MEM|8` wave memory, call `smart-vault remember` (`st remember`) or `smart-vault mesh ask` / `smart-vault mesh teach`.

## Core CLI & MCP Entrypoints

- `smart-vault doctor` — vault readiness, locks/journals, `st` availability, `st memnet` route count, `mempalace` status.
- `smart-vault scan` — parallel `rayon` scan of markdown pages, inbox items, and `.raw/` files.
- `smart-vault lint [--format json|markdown]` — parallel CSR graph lint (broken links, orphans, missing frontmatter, empty sections, duplicates, Tarjan SCC cycles, PageRank hubs).
- `smart-vault index` — build parallel BM25+ inverted index (`.vault-meta/bm25/index.json`).
- `smart-vault query "<text>" [--mesh] [--palace] [--top 8]` — parallel BM25+ search + optional MemNet wave cache + MemPalace drawers.
- `smart-vault tree [--mode ai|classic|quantum|json]` — `8b-is/smart-tree` directory view.
- `smart-vault remember [--level expert] [--check]` — `st remember` `MEM|8` wave sync.
- `smart-vault mesh <status|routes|share|ask|teach>` — live `MemNet` wave mesh & folder sharing.
- `smart-vault palace <status|wake-up|search|mine|export|daemon>` — local `MemPalace` verbatim spatial memory.
- `smart-vault mcp` — stdio JSON-RPC 2.0 MCP server (`vault_*` + `mesh_*` + `palace_*` tools).
- `smart-vault transaction <inspect|apply|recover>` — SHA-256 approval-pinned atomic transactions under `fs2` lock.
- `smart-vault sign <binary> [--identity <id>] [--write-sha256]` / `verify-binary <binary> [--sha256 <hex>]` — macOS `codesign` + constant-time SHA-256 verification.
- `smart-vault setup-host --host <claude|codex|opencode|gemini|antigravity|cursor|windsurf|zcode|all> [--apply]` — link `skills/` into any AI host.
- `smart-vault setup-global [--apply]` — automated signed M1 Antigravity IDE+CLI global MCP + hook + skill setup (see [M1 guide](docs/m1-antigravity-mempalace-guide.md)).

## Verification

Run `make check-nostd`, `make lint`, and `make test` (`cargo test --workspace` + contracts + package validation) after any change.
