# smart-vault Copilot instructions

Read `AGENTS.md` before proposing changes. `smart-vault` is a 100% pure-Rust, agent-agnostic Obsidian knowledge engine with a `#![no_std]` + `aarch64` NEON/ASM + CSR graph core wired into `8b-is/smart-tree`, `MemNet`, and `MemPalace`.

When editing:
- Keep skill frontmatter to `name` and single-line `description`.
- Keep all code pure Rust (no Python or Bash scripts) and keep `crates/smart-vault-core` strictly `#![no_std]`.
- Route shared vault mutations through `src/transaction.rs` approval-hashed bundles.
- Run `make fmt`, `make check-nostd`, `make lint`, and `make test` before wrapping up.

Made with `<3` from `8b.is-crew`.
