---
name: wiki-mode
description: "Read or configure the vault filing methodology and suggest destinations for planned knowledge creation under Generic, LYT, PARA, or Zettelkasten. Use for wiki mode, methodology mode, what is my vault mode, set vault mode, switch to PARA, use LYT, Zettelkasten setup, change mode, configure mode, or methodology routing. This skill does not save content or migrate notes."
---

# Route by vault methodology

This skill returns filing suggestions. It does not write knowledge pages or
move existing notes. If `.vault-meta/mode.json` is absent, use `generic`. If
the file exists but is invalid, fail closed and repair it through a reviewed
configuration operation before suggesting routes; never silently substitute
Generic for corrupt user configuration.

Resolve the vault explicitly with `smart-vault`:

```bash
smart-vault mode --vault "$VAULT" get
```

The helper validates the selected vault, confines paths, sanitizes names, and
prints a suggestion only. A calling skill may override the suggestion when the
user supplies a more specific project, area, MOC, or parent note, but it must
still apply its eventual writes through one operation transaction.

| Mode | Routing intent |
|---|---|
| `generic` | Type-based folders such as sources, entities, concepts, and sessions. |
| `lyt` | Atomic notes under `wiki/notes/`, connected through MOCs. |
| `para` | Projects, Areas, Resources, or Archives chosen by actionability. |
| `zettelkasten` | Flat atomic notes with time-sortable, collision-resistant identifiers and explicit links. |

Use the templates as structural guidance: [LYT atomic](templates/lyt/atomic-template.md),
[LYT MOC](templates/lyt/moc-template.md), [PARA area](templates/para/area-template.md),
[PARA project](templates/para/project-template.md),
[PARA resource](templates/para/resource-template.md), or
[Zettelkasten atomic](templates/zettel/atomic-template.md). They use canonical
`type` values documented in `WIKI.md` and a separate flat
`methodology_role` property:

| Template | Type | Methodology role |
|---|---|---|
| LYT atomic | `concept` | `atomic` |
| LYT MOC | `meta` | `moc` |
| PARA area | `concept` | `area` |
| PARA project | `entity` | `project` |
| PARA resource collection | `concept` | `resource` |
| Zettelkasten atomic | `concept` | `zettel` |

A resource template describes a topic collection. Use `source` instead when the
new page represents a particular captured source. The methodology role does not
add a router type: pass the canonical type and respect the chosen project, area,
or MOC destination. Structural MOCs use an explicit destination; `meta` is not a
routable new-note type. Existing user pages, custom type vocabularies, and saved
Dataview queries remain unchanged. Any requested migration needs its own reviewed
plan; switching mode never rewrites existing notes.

## Change mode

A mode change is one configuration operation, dry-run first:

1. Read the current `.vault-meta/mode.json`, or start from the helper's default
   config when it is absent.
2. Validate the requested mode as exactly `generic`, `lyt`, `para`, or
   `zettelkasten`. Preserve the other mode-specific settings.
3. Choose and retain a pinned UTC timestamp and operation ID for both commands.
4. Preview the canonical configuration transaction; the command records the
   current target hash and limits the write to `.vault-meta/mode.json`:

   ```bash
   smart-vault mode --vault "$VAULT" set "$MODE"
   ```

5. Show the old mode, new mode, exact changed path, and complete preview.
6. Apply after review:

   ```bash
   smart-vault mode --vault "$VAULT" set "$MODE" --apply
   ```

Follow the [operation transaction contract](../wiki/references/operation-transactions.md).

Changing mode affects routing for future operations only. Never bulk-create
folders, move notes, rewrite wikilinks, or migrate existing pages as a side
effect. If migration is later requested, plan and review it as a distinct
operation with its own hashes and transaction.

## Checkpoint

Observe the user's current structure, think about how they retrieve and act on
notes, verify a few proposed routes before changing configuration, and grow by
revisiting the mode only when real filing friction appears.
