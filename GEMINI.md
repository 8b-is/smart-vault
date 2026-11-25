# smart-vault: Gemini instructions

Read [AGENTS.md](AGENTS.md) as the canonical host-neutral contract. Skills live in `skills/<name>/SKILL.md`, the `#![no_std]` graph/ASM core lives in `crates/smart-vault-core/`, and the workspace runtime lives in `src/`.

Install skill discovery with:

```bash
smart-vault setup-host --host gemini
smart-vault setup-host --host gemini --apply
```

Or run `smart-vault setup-global --apply` for automated signed installation across Antigravity IDE & CLI, or wire `smart-vault mcp` manually:

```json
{
  "mcpServers": {
    "smart-vault": { "command": "smart-vault", "args": ["mcp"] }
  }
}
```

Made with `<3` from `8b.is-crew`.
