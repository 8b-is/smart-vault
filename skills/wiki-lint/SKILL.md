---
name: wiki-lint
description: "Run a deterministic, read-only health check on an Obsidian wiki using the parallel Rust smart-vault lint engine. Use for lint, vault health check, audit wiki health, find orphans, find dead links, frontmatter audit, provenance audit, or wiki audit."
---

# Lint the wiki

Use the parallel Rust `smart-vault` lint engine (`smart-vault lint` or MCP `vault_lint`) as the source of truth. Lint observes vault state; it does not create reports, dashboards, canvases, stubs, or fixes.

## Run

Resolve the user vault, then run one of:

```bash
smart-vault lint --vault "$VAULT"
smart-vault lint --vault "$VAULT" --format markdown
```

The deterministic parser understands Obsidian wikilinks and embeds, Markdown links, aliases, heading and block fragments (`#Heading`, `#^block`), escaped table aliases (`\|`), inline code, and fenced code blocks. It respects `.vault-meta/lint-allowlist.json` and reports `broken_wikilink`, `ambiguous_wikilink`, `broken_markdown_link`, `broken_fragment`, `missing_frontmatter`, `empty_section`, `duplicate_title`, and `orphan_page`.

## Repair is a separate operation

Never auto-fix a lint result. After the user chooses specific findings to repair, assemble one `smart-vault.transaction.v1` bundle, inspect its SHA-256 plan via `smart-vault transaction inspect`, and apply only after review with `smart-vault transaction apply`.
