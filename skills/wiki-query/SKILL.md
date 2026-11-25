---
name: wiki-query
description: "Answer an explicitly vault-scoped question from an Obsidian wiki without changing it, powered by smart-vault parallel BM25+ search and MemNet wave memory. Use when the user selects the vault as the evidence source."
---

# Query the wiki

Answer from the selected vault and leave every vault file unchanged. Treat `wiki/hot.md` as orientation, not as evidence by itself.

## Retrieve

1. Resolve the vault explicitly (`smart-vault doctor --vault "$VAULT"` or MCP `vault_doctor`).
2. Read `wiki/hot.md` and `wiki/index.md` for Quick answers, or run parallel Rust BM25+ search (and optional MemNet `MEM|8` wave lookup) for Standard/Deep queries:

```bash
smart-vault query --vault "$VAULT" "$QUERY" --top 8
smart-vault query --vault "$VAULT" "$QUERY" --top 8 --mesh
```

Or via MCP tool `vault_query` (and `vault_tree` for `8b-is/smart-tree` structure context).

## Answer

- Lead with the direct answer, citing exact wikilinks (`[[Page#Heading]]`).
- If the vault cannot answer, name the missing evidence and suggest `wiki-ingest` or `autoresearch`.
- Optionally teach confirmed stable Q&A pairs to local/LAN `MemNet` wave memory with explicit user consent (`smart-vault mesh teach "$QUERY" "$ANSWER"`).
