# Install `smart-vault` (`8b-is/smart-vault`)

`smart-vault` (`sv`) separates product binaries/skills cleanly from your user-owned Obsidian vaults:

1. **The product runtime** — single statically optimized Rust binary (`smart-vault`, short alias `sv`), zero-alloc `#![no_std]` kernel (`crates/smart-vault-core`), portable `skills/`, and agent host configs.
2. **A user Obsidian vault** — separate directory holding `.smart-vault.json` (or legacy `.memvault.json` / `.claude-obsidian.json`), `inbox/`, `.raw/`, `wiki/`, and `.vault-meta/`.

Never treat the source clone or plugin cache directory as a user vault.

---

## Requirements

- **Zero Python or Bash runtime required** for core vault operations, indexing, CSR graph linting, transactions, hooks, or the 15-tool MCP server.
- **Rust toolchain (`>= 1.75`)** only if building from source (pre-built signed `aarch64-apple-darwin` release archives are published on [GitHub Releases](https://github.com/8b-is/smart-vault/releases)).
- **Optional ecosystem binaries** (graceful pure-Rust fallback when absent):
  - `st` (`8b-is/smart-tree`) for AI-compressed trees and `.m8` wave checkpoints (`MemNet` `st memnet`).
  - `mempalace` / `mempalace-mcp` for spatial drawer search, mining, and daemon acceleration.
- **Obsidian** when browsing or editing the resulting vault visually.

---

## Option 1: Install Pre-Built Signed Release Binary (macOS Apple Silicon)

Download the tarball and `.sha256` checksum from [GitHub Releases](https://github.com/8b-is/smart-vault/releases), verify the archive digest, extract, and verify the binary signature:

```bash
curl -fsSLO https://github.com/8b-is/smart-vault/releases/latest/download/smart-vault-v4.3.0-aarch64-apple-darwin.tar.gz
curl -fsSLO https://github.com/8b-is/smart-vault/releases/latest/download/smart-vault-v4.3.0-aarch64-apple-darwin.tar.gz.sha256
shasum -a 256 -c smart-vault-v4.3.0-aarch64-apple-darwin.tar.gz.sha256

mkdir -p ~/.local/bin
tar -xzf smart-vault-v4.3.0-aarch64-apple-darwin.tar.gz -C ~/.local/bin
ln -sf ~/.local/bin/smart-vault ~/.local/bin/sv

codesign --verify --deep --strict --verbose=2 ~/.local/bin/smart-vault
smart-vault verify-binary ~/.local/bin/smart-vault --sha256 "$(awk '{print $1}' ~/.local/bin/smart-vault.sha256)"
```

---

## Option 2: Build & Install from Source

```bash
git clone https://github.com/8b-is/smart-vault.git
cd smart-vault

# Compile LTO-optimized native release binary and install to ~/.cargo/bin
cargo build --release
cargo install --path . --locked

# Sign & emit a .sha256 sidecar (macOS codesign + constant-time SHA-256)
cargo run --release --quiet -- sign ~/.cargo/bin/smart-vault --write-sha256
```

---

## Option 3: Automated M1 Global Setup (Antigravity IDE + CLI + MemPalace)

See [M1 Antigravity + MemPalace Guide](m1-antigravity-mempalace-guide.md) for the complete walkthrough:

```bash
# Dry-run preview (writes nothing)
smart-vault setup-global --repo-root /path/to/smart-vault

# Apply globally: installs signed ~/.local/bin/smart-vault + sv symlink,
# merges ~/.gemini/config/mcp_config.json & ~/.gemini/settings.json (preserving 0600 perms),
# and links ~/.gemini/config/skills/smart-vault
smart-vault setup-global --repo-root /path/to/smart-vault --apply
```

---

## Portable Agent Host Skill Linking (`setup-host`)

Preview linking canonical `skills/` directories into any supported agent host (writes nothing until `--apply` is passed):

```bash
smart-vault setup-host --host all
smart-vault setup-host --host gemini --apply
smart-vault setup-host --host codex --apply
smart-vault setup-host --host claude --apply
smart-vault setup-host --host opencode --apply
smart-vault setup-host --host zcode --apply
smart-vault setup-host --host cursor --workspace /path/to/project --apply
smart-vault setup-host --host windsurf --workspace /path/to/project --apply
```

Add the stdio MCP server in any MCP host configuration:

```json
{
  "mcpServers": {
    "smart-vault": {
      "command": "smart-vault",
      "args": ["mcp"]
    }
  }
}
```

---

## Create or Adopt an Obsidian Vault

All setup commands preview changes by default and require explicit `--apply`:

```bash
# New vault
smart-vault init --vault /path/to/my-vault
smart-vault init --vault /path/to/my-vault --apply

# Adopt an existing Obsidian vault non-destructively
smart-vault adopt --vault /path/to/existing-vault
smart-vault adopt --vault /path/to/existing-vault --apply
```

The scaffolded vault includes:
- `.smart-vault.json` — workspace manifest (`smart-vault.workspace.v1`)
- `.gitignore` — excludes `.vault-meta/` runtime state and Obsidian workspace caches
- `inbox/` — source intake folder
- `.raw/` — immutable create-only source payloads + `.manifest.json`
- `wiki/` — `index.md`, `log.md`, `hot.md`, `overview.md`, and structured note directories
- `.obsidian/` — minimal non-destructive Obsidian defaults

---

## Vault Selection Precedence

`smart-vault` resolves the active vault in this order and fails closed when ambiguous or inside the product source root:

1. `--vault <path>`
2. `SMART_VAULT_ROOT`
3. Legacy `MEMVAULT_ROOT` / `CLAUDE_OBSIDIAN_VAULT`
4. Nearest `.smart-vault.json`, `.memvault.json`, `.claude-obsidian.json`, `.vault-meta/`, or `wiki/index.md` walking upward from `CWD`

Verify vault readiness anytime:

```bash
smart-vault doctor --vault /path/to/my-vault
smart-vault lint --vault /path/to/my-vault --format markdown
```

---

## Transactions, Recovery & Checkpoints

All shared vault writes flow through SHA-256 approval-pinned bundles locked via `fs2` on `.vault-meta/mutation.lock`:

```bash
# Inspect plan and compute approval SHA-256
smart-vault transaction inspect bundle.json --vault /path/to/my-vault

# Apply only after reviewing the exact plan digest
smart-vault transaction apply bundle.json --vault /path/to/my-vault --approved-plan-sha256 <sha256>

# Inspect or recover incomplete pre-image journals after an interruption
smart-vault transaction recover --vault /path/to/my-vault
smart-vault transaction recover --vault /path/to/my-vault --apply
```

---

## Troubleshooting

| Symptom | Fix |
|---|---|
| `no vault markers found` | Pass `--vault <path>`, set `SMART_VAULT_ROOT`, or `cd` into an initialized vault. |
| Binary signature check fails after copying | Re-sign with `smart-vault sign ~/.local/bin/smart-vault --write-sha256` and re-run `smart-vault verify-binary`. |
| `BM25 index not found` | Run `smart-vault index --vault <path>` (or rely on auto-build with `smart-vault query`). |
| `PLAN_CHANGED` / hash mismatch | Re-run `transaction inspect` after any intervening edit to compute a fresh `approved_plan_sha256`. |
| Windows writes limited | Use WSL for vault mutations; see [Windows and WSL guide](windows-wsl.md). |

Made with `<3` from **8b.is-crew**.
