---
name: wiki-retrieve
description: "Build and query the parallel Rust BM25+ retrieval index and MemNet MEM|8 wave cache for an Obsidian vault. Use for retrieve, hybrid retrieval, BM25, vault search, wave memory, or offline retrieval evaluation."
---

# Retrieve relevant passages

`smart-vault` builds a parallel (`rayon`) Unicode-aware BM25+ inverted index under `.vault-meta/bm25/index.json` (with NFD diacritic folding and CJK 1/2/3-grams), enforces live SHA-256 freshness before serving results, and optionally checks local/LAN `MemNet` `MEM|8` wave packets.

## Build & Query

```bash
smart-vault index --vault "$VAULT"
smart-vault query --vault "$VAULT" "$QUERY" --top 8
smart-vault query --vault "$VAULT" "$QUERY" --top 8 --mesh
smart-vault bench --vault "$VAULT" --queries tests/fixtures/retrieval-quality/queries.json --top 5
```

## Smart Tree & MemNet Sync

```bash
smart-vault tree --vault "$VAULT" --mode ai
smart-vault remember --vault "$VAULT" --level expert
smart-vault mesh status
```
