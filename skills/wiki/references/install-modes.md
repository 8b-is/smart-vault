# Supported installation modes

Invoke the pure-Rust `smart-vault` binary or MCP server directly:

```bash
smart-vault --help
```

## Claude marketplace plugin

Install the plugin through its marketplace and invoke namespaced skills such as
`/claude-obsidian:wiki`. Plugin assets live in an ephemeral cache. Mutable vault
state must resolve from the project, `--vault`, or workspace configuration,
never `${CLAUDE_PLUGIN_ROOT}`.

## Local Claude plugin development

```bash
claude --plugin-dir /path/to/claude-obsidian
```

Skills remain namespaced. Run Claude from the user vault, not the plugin source
directory, or configure the vault explicitly.

## Portable Agent Skills

Use `smart-vault setup-host` or link each `skills/<name>/` directory at the
host's direct `<skill-root>/<name>/SKILL.md` discovery path. Codex, OpenCode,
Gemini, Antigravity, Cursor, Windsurf, ZCode, and other Agent Skills hosts consume the same `SKILL.md` files.

## Product clone

A source clone is product/development code, not an automatically installed
plugin and not the recommended user vault. Create a separate vault:

```bash
smart-vault init --vault /path/to/vault
smart-vault init --vault /path/to/vault --apply
```

Adopt an existing Obsidian vault non-destructively:

```bash
smart-vault adopt --vault /path/to/vault
smart-vault adopt --vault /path/to/vault --apply
```

Both commands default to dry-run. New user vaults receive no upstream product
remote.
