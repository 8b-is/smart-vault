---
name: wiki
description: "Initialize, adopt, and route work for a separate Obsidian knowledge vault through the pure-Rust smart-vault core (#![no_std] CSR graph + aarch64 NEON/ASM, wired into 8b-is/smart-tree, MemNet, and MemPalace). Use for vault setup, scaffolding, workspace selection, cross-project configuration, or choosing the correct wiki sub-skill. Triggers: /wiki, set up wiki, scaffold vault, create knowledge base, adopt this vault, Obsidian vault, second brain setup, persistent wiki."
---

# Wiki orchestration

Treat the installed product as code and the selected user vault as data. Never use
the plugin/product root as a vault, even when the current directory happens to be
the product checkout.

Invoke the pure-Rust `smart-vault` CLI directly (or via MCP tools `vault_doctor`, `vault_tree`, `vault_query`, `vault_lint`, `vault_remember`, `mesh_*`, `palace_*`):

```bash
smart-vault --help
```

Resolve a vault in this order: explicit `--vault`,
`SMART_VAULT_ROOT` (or `MEMVAULT_ROOT` / `CLAUDE_OBSIDIAN_VAULT`), the nearest `.smart-vault.json`, `.memvault.json`, or `.claude-obsidian.json`, then an
unambiguous initialized vault at or above the current directory. Fail closed when
selection is missing or ambiguous.

Baseline setup requires no network egress. Do not fetch templates, plugins, or
remote content unless the user separately approves the destinations and budget.

## Set up a vault

Use the deterministic setup commands. Both are dry-run by default.

For a new, separate vault:

```bash
smart-vault init --vault /absolute/path/to/vault
smart-vault init --vault /absolute/path/to/vault --apply
```

For an existing Obsidian vault:

```bash
smart-vault adopt --vault /absolute/path/to/vault
smart-vault adopt --vault /absolute/path/to/vault --apply
```

Before `--apply`, show the selected path and changed-path preview, then pass
the emitted `approved_plan_sha256` unchanged. Do not use
`--force` unless the user has reviewed the conflicts and explicitly approved
replacement. Setup is non-destructive by default and creates no upstream Git
remote.

If the user asks for a domain-specific scaffold, establish the baseline first,
then read [modes.md](references/modes.md). Draft the additional pages and
configuration as one operation-level transaction. Never mutate vault files with
host Write/Edit tools or an Obsidian transport.

## Route operations

Route the user's intent without silently broadening it:

| Intent | Skill |
|---|---|
| Ingest supplied sources | `wiki-ingest` |
| Answer from existing vault knowledge | `wiki-query` |
| Save a specific conversation result | `save` |
| Research the public web under a budget | `autoresearch` |
| Check vault health | `wiki-lint` |
| Draft a cited glossary or consolidate overlapping notes | `wiki-ingest`, with an explicitly reviewed scope |
| Roll up log entries | `wiki-fold` |
| Work with a canvas | `canvas` |

Query is read-only. Persistence from a query must be an explicit, separately
scoped Save operation. Never capture a transcript or update the hot cache merely
because a session ended.

## Mutation contract

Read [operation-transactions.md](references/operation-transactions.md) before
any custom scaffold or mutation. One logical operation must produce one inspected
and recoverable `claude-obsidian.transaction.v1` bundle. Parallel agents may
return drafts and evidence only; the orchestrator merges them and applies once.
Every canonical page create or removal includes an active index or MOC update
in that bundle; update the overview only when the stable high-level picture
changed. Raw source payloads are create-only. There are no automatic commits.

Use [provenance.md](references/provenance.md) when initializing or changing
source and claim ledgers. Unsupported evidence stays unsupported; never invent a
source, quote, date, locator, or confidence.

After a successful apply, report the operation ID and exact changed paths. Check `smart-vault tree --vault /absolute/path/to/vault --mode ai` or sync salient `MEM|8` waves with `smart-vault remember --vault /absolute/path/to/vault`.

On a conflict, re-read and rebuild. On interruption, use `smart-vault transaction recover --apply`.
Reuse an operation ID only with the identical bundle.

## Installation context

Read [install-modes.md](references/install-modes.md) when installation or host
behavior matters. Hooks are optional adapters; portable behavior lives in the
core and skills.

## Conditional references

Read only the reference needed for the current request:

- [frontmatter.md](references/frontmatter.md) when defining or adopting a
  property schema;
- [css-snippets.md](references/css-snippets.md) for requested Obsidian visual
  customization;
- [git-setup.md](references/git-setup.md) for explicit local Git or checkpoint
  setup;
- [plugins.md](references/plugins.md) when evaluating optional Obsidian
  integrations;
- [mcp-setup.md](references/mcp-setup.md) when the user asks to evaluate an
  external read transport;
- [rest-api.md](references/rest-api.md) only when the user explicitly has or
  requests the Local REST API adapter;
- [dataview.md](references/dataview.md) for an existing optional Dataview setup;
- [knowledge-recipes.md](references/knowledge-recipes.md) for cited glossary or
  consolidation work;
- [coordination.md](references/coordination.md) for external editors or multiple
  machines;
- [native-conformance.md](../../docs/native-conformance.md) for versioned native
  acceptance evidence;
- [capacity-and-recovery.md](../../docs/capacity-and-recovery.md) for size limits
  or recoverable legacy compaction;
- [upstream-references.md](../../docs/upstream-references.md) when reconciling
  pinned external syntax or design references.
- [model-guidance.md](references/model-guidance.md) when evaluating a model,
  changing a generation adapter, or validating host-specific prompting.

## Think, verify, grow

Before applying, pause once: observe existing state, verify the vault selection
and evidence, then choose the smallest reversible operation that satisfies the
request. Afterward, report uncertainty and the next useful improvement without
performing it automatically.
