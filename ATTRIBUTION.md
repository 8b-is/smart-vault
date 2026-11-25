# Attributions

`smart-vault` (`8b-is/smart-vault`, made with `<3` from **8b.is-crew**) is a pure-Rust, agent-agnostic Obsidian knowledge engine evolved from `claude-obsidian` and wired into `8b-is/smart-tree`, `MemNet`, and `MemPalace`. The following upstream foundations, architectures, open-source crates, and contributors informed or power its design.

---

## 8b.is-crew · Smart Tree, MemNet & Pure-Rust `#![no_std]` Core

**Authors:** 8b.is-crew (`8bit-wraith`, `Claude`, `Omni`, `peterlodri-sec`, and the 8b.is team)
**Repositories:**
- https://github.com/8b-is/smart-tree
- https://github.com/peterlodri-sec/claude-obsidian

**Use:** 100% pure-Rust `smart-vault` workspace (`crates/smart-vault-core/` `#![no_std]` kernel + `src/`), `aarch64` inline ASM & NEON SIMD byte kernels, CSR graph representation with iterative Tarjan SCC & PageRank hub scoring, `rayon` parallel BM25+ indexing & graph linting, `mimalloc` global allocator, macOS `codesign` + constant-time SHA-256 verification, SHA-256 approval-pinned transactions under `fs2` locking, stdio JSON-RPC 2.0 MCP server (15 tools), lifecycle hook integration, automated M1 global installer (`setup-global`), and native `st` + `st memnet` (`MEM|8` wave persistence, Q&A cache offloading, and peer folder sharing) integration.

---

## MemPalace

**Architecture:** Verbatim spatial memory palace organized into Wings, Rooms, and Drawers backed by ChromaDB, HNSW vector indexing, and SQLite (`~/.config/mempalace/palace` / `~/.mempalace/palace`).
**Integration in `smart-vault`:** Local CLI bridge (`smart-vault palace status|wake-up|search|mine|export|daemon`), hybrid query fusion (`smart-vault query --palace`), MCP tools (`palace_status`, `palace_wake_up`, `palace_search`, `palace_mine`), lifecycle hook forwarding (`smart-vault hook <event> --palace`), and `mempalace-mcp` / `mempalace-light-mcp` registration in `smart-vault setup-global`.

`smart-vault` invokes locally installed `mempalace` executables as an external process boundary with graceful degradation when absent; no third-party MemPalace source code is bundled in this repository.

---

## Pure-Rust Ecosystem Crates

`smart-vault` is compiled as a single pure-Rust binary (declared in [Cargo.toml](Cargo.toml)) built atop a zero-dependency `#![no_std]` kernel ([crates/smart-vault-core/Cargo.toml](crates/smart-vault-core/Cargo.toml)) and these open-source crates:

| Crate | Role in `smart-vault` |
|---|---|
| `smart-vault-core` (`#![no_std]`) | Zero-allocation CSR graph, iterative Tarjan SCC, PageRank, `aarch64` NEON/ASM primitives (`ct_eq_bytes`, `ctz_u64`, `simd_contains_byte`, `simd_ascii_lowercase_in_place`), zero-alloc Markdown scanner, IEEE-754 BM25+ math, stack `TopK` |
| `mimalloc` | Default high-throughput parallel allocator for `rayon` indexing, linting, and MCP serving |
| `clap` | CLI argument, subcommand, and environment-variable parsing (`derive`, `env`) |
| `rayon` | Data-parallel vault scanning, graph linting, and BM25+ inverted-index construction |
| `memchr` | SIMD-accelerated byte scanning for Markdown inline-code and link parsing |
| `serde` & `serde_json` | Deterministic JSON serialization for manifests, transaction journals, CLI reports, and MCP JSON-RPC 2.0 |
| `sha2` & `hex` | SHA-256 transaction approval hashes, pre-image verification, `.raw/` content addressing, and binary `.sha256` verification |
| `fs2` | Exclusive advisory filesystem locking over `.vault-meta/mutation.lock` |
| `rustc-hash` | High-speed `FxHashMap` lookup tables in graph linting and BM25+ indexing/scoring |
| `unicode-normalization` | Unicode NFD decomposition and combining-mark folding for multilingual BM25+ retrieval |
| `walkdir` | Deterministic vault directory traversal |
| `chrono` | UTC timestamps for journals, BM25+ index metadata, and vault templates |
| `thiserror` | Structured, error-coded `MemVaultError` diagnostics |
| `criterion` & `tempfile` | Criterion microbenchmarks ([benches/vault_bench.rs](benches/vault_bench.rs)) and hermetic integration tests ([tests/integration_tests.rs](tests/integration_tests.rs)) |

---

## Obsidian & `kepano/obsidian-skills`

**Sources:**
- https://obsidian.md
- https://github.com/kepano/obsidian-skills

**Use:** Design reference for Obsidian Flavored Markdown (`[[wikilinks]]`, embeds, callouts, block IDs), Obsidian Canvas, and Obsidian Bases syntax. No upstream code is copied; pinned reference reviews and intentional divergences are documented in [config/upstream-references.json](config/upstream-references.json) and [docs/upstream-references.md](docs/upstream-references.md).

---

## LLM Wiki Pattern

**Author:** Andrej Karpathy
**Source:** https://github.com/karpathy
**Use:** The core architecture of claude-obsidian (using an LLM to build and maintain a structured wiki from raw sources) is based on the LLM Wiki pattern Karpathy described publicly. claude-obsidian is an independent implementation; no code or content from Karpathy's repositories was copied.

---

## ITS CSS Snippets

**Author:** SlRvb
**Source:** https://github.com/SlRvb/Obsidian--ITS-Theme
**License:** GPL-2.0
**Files:**
- `.obsidian/snippets/ITS-Dataview-Cards.css`
- `.obsidian/snippets/ITS-Image-Adjustments.css`

These snippets exist only in the private contributor-vault state. They are not
selected into the deterministic public product artifact and are not installed
by the vault template. The project-owned `templates/vault/.obsidian/snippets/vault-colors.css`
is the only snippet distributed by claude-obsidian.

---

## Historical contributor-vault plugins

The source repository has contained the following Obsidian community plugins in
its contributor-vault state. They are not included in the deterministic public
product artifact or installed by the vault template. Install and review them
through Obsidian under their respective licenses.

| Plugin | Author | Repository |
|--------|--------|-----------|
| Calendar | Liam Cain | https://github.com/liamcain/obsidian-calendar-plugin |
| Thino | Boninall (Quorafind) | https://github.com/Quorafind/Obsidian-Thino |
| Obsidian Excalidraw | Zsolt Viczian | https://github.com/zsviczian/obsidian-excalidraw-plugin |
| Obsidian Banners | Danny Hernandez | https://github.com/noatpad/obsidian-banners |

`scripts/setup-vault.sh` does not download unverified plugin binaries. It prints
manual installation guidance instead.

---

## Adopted contributor designs

Two community pull requests diagnosed retrieval bugs and proposed fixes that
the v2.0.0 refoundation implemented in near-identical form. The pull requests
were not merged as submitted because the surrounding files were rewritten, but
the designs are theirs.

| Change | Contributor | Pull request |
|--------|-------------|--------------|
| Nomic `search_query` and `search_document` task prefixes in the reranker, including the `with_task_prefix` helper and cache-scheme invalidation | maartengoet | https://github.com/AgriciDaniel/claude-obsidian/pull/77 |
| BM25 fallback order preserved in reranker no-op paths (`bm25_score` before `score`) | vinsocci | https://github.com/AgriciDaniel/claude-obsidian/pull/62 |

Version 3.0.0 also draws on the following diagnosed fixes and proposals. These
credits do not imply that a pull request was merged or accepted without
revision.

| Change or design input | Contributor | Pull request |
|--------|-------------|--------------|
| Split encoded Markdown destinations before decoding their path | Ychris12138 | https://github.com/AgriciDaniel/claude-obsidian/pull/194 |
| Canvas `_index.md`, title-preserving routes and legacy journal compatibility | Marcos64s | https://github.com/AgriciDaniel/claude-obsidian/pull/199 |
| Haiku 5.5 model and cache-floor migration, revised here with explicit thinking and output limits | hphsu | https://github.com/AgriciDaniel/claude-obsidian/pull/195 |
| Optional local Ollama prefix tier, independently reimplemented here with bounded transport and local-model preflight | vinsocci | https://github.com/AgriciDaniel/claude-obsidian/pull/63 |
| Dataview queries use vault-relative content paths | miraclemanza | https://github.com/AgriciDaniel/claude-obsidian/pull/46 |

These community pull requests were merged as submitted.

| Change | Contributor | Pull request |
|--------|-------------|--------------|
| ZCode host adapter for portable skill discovery | Lion-1209 | https://github.com/AgriciDaniel/claude-obsidian/pull/146 |
| One page-type vocabulary, so the router files every documented type | HectorBernstorff | https://github.com/AgriciDaniel/claude-obsidian/pull/149 |
| Lint ignores gitignored candidates when resolving ambiguous wikilinks | Packetslave | https://github.com/AgriciDaniel/claude-obsidian/pull/159 |
| Ingest prefers Markdown links for citable URLs | CooperSheroy | https://github.com/AgriciDaniel/claude-obsidian/pull/176 |
| UTF-8 test output so Windows test runs do not crash | yotamleo | https://github.com/AgriciDaniel/claude-obsidian/pull/178 |
| Contextual prefixes parse CRLF frontmatter | ketpatil77 | https://github.com/AgriciDaniel/claude-obsidian/pull/180 |
| Scaffolded vaults pinned to the absolute wikilink format | tilusnet | https://github.com/AgriciDaniel/claude-obsidian/pull/182 |

Version 2.2.0 also draws on these pull requests, implemented here independently.

| Change | Contributor | Pull request |
|--------|-------------|--------------|
| The repository root ignores `.mcp.json`, a possible credential carrier | GPezzuti | https://github.com/AgriciDaniel/claude-obsidian/pull/44 |
| `stop_status` reads large valid journals instead of reporting them unreadable | STiFLeR7 | https://github.com/AgriciDaniel/claude-obsidian/pull/172 |
| `checkpoint` honors Git's false values for `core.filemode` | STiFLeR7 | https://github.com/AgriciDaniel/claude-obsidian/pull/173 |

These issue reports, several with a diagnosis or a tested patch, led to fixes
in 3.0.0.

| Report | Reporter | Issue |
|--------|----------|-------|
| Lint passed a flat vault clean with 0 pages scanned | RoelKluin | https://github.com/AgriciDaniel/claude-obsidian/issues/200 |
| Decoding before the fragment split broke `%23`-encoded links | threnard | https://github.com/AgriciDaniel/claude-obsidian/issues/193 |
| Lint reported existing hidden `.raw` targets as missing files | szrenwei | https://github.com/AgriciDaniel/claude-obsidian/issues/191 |
| SessionStart and Stop hooks timed out on Windows | ppuri2 | https://github.com/AgriciDaniel/claude-obsidian/issues/168 |
| CLI transport detection ignored whether this vault was reachable | cosmos75 | https://github.com/AgriciDaniel/claude-obsidian/issues/141 |
| Glossary proposal | simonc | https://github.com/AgriciDaniel/claude-obsidian/issues/25 |

Pinned ecosystem references, licenses and intentional differences are recorded
in [config/upstream-references.json](config/upstream-references.json). The
3.0.0 original examples do not incorporate restricted third-party plugin
implementations or personal contributor vaults.

---

## claude-obsidian

**Author:** AgriciDaniel / AI Marketing Hub
**License:** MIT (see [LICENSE](LICENSE))
**Repository (public canonical):** https://github.com/AgriciDaniel/claude-obsidian
**Community early-access mirror (Pro):** https://github.com/AI-Marketing-Hub
