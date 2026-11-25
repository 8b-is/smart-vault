# Global M1 (64 GB) Setup Guide: `smart-vault` + `Smart Tree` + `MemNet` + `MemPalace` + Antigravity IDE & CLI

This machine runs the full 4-layer local-first memory stack globally across **Antigravity IDE** and **Antigravity CLI**:

| Layer | Engine | Role |
|---|---|---|
| 1. Vault Knowledge Graph | **Obsidian + `smart-vault` (`sv`)** (Rust `#![no_std]` + ASM + CSR) | Source-cited Markdown wiki, parallel CSR graph lint (Tarjan SCC + PageRank), BM25+ search, SHA-256 atomic transactions |
| 2. Structural & Wave Memory | **`8b-is/smart-tree` (`st`)** | AI tree summaries (`smart-vault tree`) & salient `.m8` wave checkpoints (`smart-vault remember`) |
| 3. Local/LAN Mesh Cache | **`MemNet` (`st memnet`)** | Zero-hop `MEM|8` wave Q&A offloading (`smart-vault mesh ask`/`teach`) & peer folder sharing |
| 4. Verbatim Spatial Palace | **`MemPalace`** (`~/.config/mempalace/palace`) | Verbatim conversation & project drawers (`smart-vault palace status`/`wake-up`/`search`/`mine`) |

---

## Automated 1-Command Global Setup (IDE + CLI)

Preview every change (dry-run, writes nothing):

```bash
smart-vault setup-global --repo-root /Users/peterlodri/obsidian
```

Apply globally (idempotent, signs the binary with `codesign` + `.sha256` sidecar, creates `.json.bak` backups before writing):

```bash
smart-vault setup-global --repo-root /Users/peterlodri/obsidian --apply
```

### What `smart-vault setup-global --apply` configures automatically

1. **Signed Global Binary + Alias**: Installs `smart-vault` into `~/.local/bin/smart-vault` (`0755`), applies macOS `codesign` + `~/.local/bin/smart-vault.sha256`, and links short alias `~/.local/bin/sv` -> `smart-vault`.
2. **Antigravity IDE + CLI MCP Servers** (`~/.gemini/config/mcp_config.json`):
   - Adds `smart-vault` (`~/.local/bin/smart-vault mcp`, replacing any legacy `memvault` entry) exposing 15 tools (`vault_*`, `mesh_*`, `palace_*`).
   - Adds `mempalace` (`~/.local/bin/mempalace-mcp`, or pass `--light-mcp` for 3-tool `mempalace-light-mcp`) alongside `smart-tree`, `memnet`, and all existing servers.
   - Creates `~/.gemini/config/mcp_config.json.bak` before modifying (preserving `0600` permissions).
3. **Global Lifecycle Hooks** (`~/.gemini/settings.json`):
   - Registers `smart-vault hook <event> --palace` (upgrading legacy `memvault` hooks) under:
     - `SessionStart` -> `smart-vault hook session-start --palace`
     - `PreCompress` -> `smart-vault hook pre-compact --palace`
     - `AfterAgent` -> `smart-vault hook stop --palace`
     - `SessionEnd` -> `smart-vault hook session-end --palace`
   - Preserves all existing hooks (including `caveman` and `shrink-hook`) without duplication.
   - Creates `~/.gemini/settings.json.bak` before modifying (preserving `0600` permissions).
4. **Global Agent Skills** (`~/.gemini/config/skills/smart-vault` -> `/Users/peterlodri/obsidian/skills`, removing any legacy `memvault` symlink).

---

## M1 64 GB Unified Memory Tip: Keep `mempalace daemon` Warm

With 1.56M+ drawers in `~/.config/mempalace/palace`, cold-loading ChromaDB + HNSW per CLI call takes several seconds, while keeping the daemon warm in M1 64 GB unified memory makes searches and mining sub-second:

```bash
smart-vault palace daemon start
smart-vault palace daemon status
```

To inspect fast read-only health without loading HNSW into a throwaway process:

```bash
smart-vault palace status         # runs fast `mempalace repair-status`
smart-vault palace wake-up        # loads L0 + L1 context
```

---

## Daily Unified Usage

```bash
# Health across Vault + Smart Tree + MemNet + MemPalace
smart-vault doctor --vault /path/to/vault

# Hybrid query across local BM25+ + MemNet wave cache + MemPalace verbatim drawers
sv query --vault /path/to/vault "topic" --mesh --palace

# Mine Obsidian wiki/ into MemPalace
smart-vault palace mine --vault /path/to/vault --wing my_vault

# Verify OS code signature + SHA-256 digest anytime
smart-vault verify-binary ~/.local/bin/smart-vault

# Run standalone or inside any Antigravity IDE / CLI session via MCP
smart-vault mcp
```

## Verify or Roll Back

```bash
which smart-vault sv && smart-vault --version
codesign --verify --deep --strict --verbose=2 ~/.local/bin/smart-vault
smart-vault verify-binary ~/.local/bin/smart-vault
python3 -m json.tool ~/.gemini/config/mcp_config.json >/dev/null
python3 -m json.tool ~/.gemini/settings.json >/dev/null

# Optional rollback from automated backups:
cp ~/.gemini/config/mcp_config.json.bak ~/.gemini/config/mcp_config.json
cp ~/.gemini/settings.json.bak ~/.gemini/settings.json
```

Made with `<3` from `8b.is-crew`.
