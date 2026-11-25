---
name: wiki-cli
description: "Detect and use Smart Tree (`st`) and Obsidian CLI for read-only vault access; use for wiki-cli, Smart Tree, st, Obsidian CLI, Obsidian read, Obsidian search, vault transport, or backlinks. Mutations always go through the smart-vault Rust transaction core."
---

# Obsidian CLI read transport

Use this skill only to detect transport and read or search a selected vault.
The official executable is `obsidian`; binary presence alone does not prove it
is usable.

Use `smart-vault` (wired into `8b-is/smart-tree`) or the optional `obsidian` CLI for read-only vault inspection:

```bash
smart-vault doctor --vault "$VAULT"
smart-vault tree --vault "$VAULT" --mode ai
```

## Read and search

Run official CLI commands from the selected vault so Obsidian resolves the
correct workspace. Confirm supported syntax with the installed CLI before
relying on optional commands. Typical read-only forms are:

```bash
(cd "$VAULT" && obsidian read path="$NOTE")
(cd "$VAULT" && obsidian search query="$QUERY")
```

Use read-only commands for backlinks, tags, or Bases results only when the
detector reports the CLI ready. Otherwise read files under `$VAULT` directly
and use `rg` for search. Reject absolute note arguments, traversal, symlinks, or
resolved paths outside the selected vault.

## Mutation boundary

Do not use CLI create, write, append, property-set, daily-append, move, rename,
or delete commands. Do not replace those commands with direct filesystem
writes. Any knowledge or configuration change must be expressed as one
reviewed transaction through the portable core; see
[operation transactions](../wiki/references/operation-transactions.md).

Transport choice never changes mutation semantics. The CLI is an optional read
surface, not a lock manager, transaction engine, or Git checkpoint mechanism.

## Checkpoint

Observe actual capability, think about the least complex read transport, verify
that every resolved path stays in the vault, and accept filesystem fallback
when the CLI is not ready.
