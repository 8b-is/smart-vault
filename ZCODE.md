# smart-vault: ZCode instructions

Read [AGENTS.md](AGENTS.md) as the canonical host-neutral contract. Skills live in `skills/<name>/SKILL.md`, the `#![no_std]` core lives in `crates/smart-vault-core/`, and the pure-Rust runtime lives in `src/`.

Install skill discovery for ZCode (`~/.zcode/skills/smart-vault`):

```bash
smart-vault setup-host --host zcode
smart-vault setup-host --host zcode --apply
```

Wire the agent-agnostic MCP server (`smart-vault mcp`) to use `vault_doctor`, `vault_lint`, `vault_query`, `vault_tree`, `vault_remember`, `mesh_*`, and `palace_*` tools natively.

Made with `<3` from `8b.is-crew`.
