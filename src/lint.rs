use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use rayon::prelude::*;
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use smart_vault_core::graph::{
    build_csr_in_place, pagerank_in_place, tarjan_scc_with_scratch, TarjanScratch,
};
use walkdir::WalkDir;

use crate::error::Result;
use crate::markdown::{parse_markdown, ParsedNote};
use crate::paths::{assert_unaliased_directory, to_posix_rel};

#[derive(Debug, Clone, Deserialize, Default)]
pub struct LintAllowlist {
    #[serde(default)]
    pub dangling_links: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct LintFinding {
    pub kind: String,
    pub severity: String,
    pub path: String,
    pub line: Option<usize>,
    pub target: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct HubScore {
    pub path: String,
    pub pagerank: f64,
    pub in_degree: u32,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct GraphMetrics {
    pub node_count: usize,
    pub edge_count: usize,
    pub scc_count: usize,
    pub cyclic_sccs: usize,
    pub largest_scc: usize,
    pub top_hubs: Vec<HubScore>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LintReport {
    pub vault_root: String,
    pub page_count: usize,
    pub issue_count: usize,
    pub findings_by_kind: BTreeMap<String, usize>,
    pub graph_metrics: GraphMetrics,
    pub findings: Vec<LintFinding>,
}

struct LoadedPage {
    rel_path: String, // e.g. "wiki/concepts/Alpha.md"
    wiki_rel: String, // e.g. "concepts/Alpha"
    stem: String,     // e.g. "Alpha"
    parsed: ParsedNote,
}

const META_HUB_PAGES: &[&str] =
    &["wiki/index.md", "wiki/overview.md", "wiki/hot.md", "wiki/log.md"];

/// Runs a parallel (`rayon`) graph & structural lint check over `<vault>/wiki`.
pub fn lint_vault(vault_root: &Path) -> Result<LintReport> {
    let wiki_dir = vault_root.join("wiki");
    if !wiki_dir.exists() {
        return Ok(LintReport {
            vault_root: vault_root.display().to_string(),
            page_count: 0,
            issue_count: 0,
            findings_by_kind: BTreeMap::new(),
            graph_metrics: GraphMetrics::default(),
            findings: Vec::new(),
        });
    }
    assert_unaliased_directory(&wiki_dir)?;

    let allowlist = load_allowlist(vault_root);

    let mut md_paths = Vec::new();
    for entry in WalkDir::new(&wiki_dir).follow_links(false).into_iter().flatten() {
        let path = entry.path();
        if entry.file_type().is_symlink() {
            continue;
        }
        if entry.file_type().is_file() && path.extension().and_then(|e| e.to_str()) == Some("md") {
            md_paths.push(path.to_path_buf());
        }
    }
    md_paths.sort();

    let pages: Vec<LoadedPage> = md_paths
        .par_iter()
        .filter_map(|path| {
            let text = fs::read_to_string(path).ok()?;
            let rel_path = to_posix_rel(vault_root, path);
            let wiki_rel = strip_wiki_prefix_and_ext(&rel_path).to_string();
            let stem =
                path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let parsed = parse_markdown(&text);
            Some(LoadedPage { rel_path, wiki_rel, stem, parsed })
        })
        .collect();

    // Build O(1) FxHashMap lookup tables across all pages
    let mut by_wiki_rel: FxHashMap<String, usize> =
        FxHashMap::with_capacity_and_hasher(pages.len(), Default::default());
    let mut by_stem: FxHashMap<String, Vec<usize>> =
        FxHashMap::with_capacity_and_hasher(pages.len(), Default::default());
    let mut by_alias: FxHashMap<String, Vec<usize>> =
        FxHashMap::with_capacity_and_hasher(pages.len() * 2, Default::default());

    for (idx, page) in pages.iter().enumerate() {
        by_wiki_rel.insert(page.wiki_rel.to_lowercase(), idx);
        by_stem.entry(page.stem.to_lowercase()).or_default().push(idx);
        if let Some(ref title) = page.parsed.frontmatter.title {
            by_alias.entry(title.to_lowercase()).or_default().push(idx);
        }
        for alias in &page.parsed.frontmatter.aliases {
            by_alias.entry(alias.to_lowercase()).or_default().push(idx);
        }
    }

    let mut has_inbound: Vec<AtomicBool> =
        (0..pages.len()).map(|_| AtomicBool::new(false)).collect();
    let ctx = LintContext {
        pages: &pages,
        allowlist: &allowlist,
        by_wiki_rel: &by_wiki_rel,
        by_stem: &by_stem,
        by_alias: &by_alias,
        vault_root,
        wiki_dir: &wiki_dir,
        has_inbound: &has_inbound,
    };

    // Validate each page's frontmatter, sections, wikilinks, and markdown links in parallel,
    // collecting both findings and directed `(src, dst)` edges for CSR graph analysis.
    type PageLintResult = (Vec<LintFinding>, Vec<(u32, u32)>);
    let page_results: Vec<PageLintResult> = pages
        .par_iter()
        .enumerate()
        .map(|(page_idx, page)| lint_single_page(page_idx, page, &ctx))
        .collect();

    let total_findings: usize = page_results.iter().map(|(f, _)| f.len()).sum();
    let total_edges: usize = page_results.iter().map(|(_, e)| e.len()).sum();
    let mut findings: Vec<LintFinding> = Vec::with_capacity(total_findings + 16);
    let mut edge_pairs: Vec<(u32, u32)> = Vec::with_capacity(total_edges);
    for (mut page_findings, page_edges) in page_results {
        findings.append(&mut page_findings);
        edge_pairs.extend_from_slice(&page_edges);
    }

    // Build zero-alloc CSR graph + run iterative Tarjan SCC and ping-pong PageRank
    let n = pages.len();
    let graph_metrics = if n > 0 {
        let mut offsets = vec![0u32; n + 1];
        let mut csr_edges = vec![0u32; edge_pairs.len()];
        let mut in_degrees = vec![0u32; n];
        let csr = build_csr_in_place(n, &edge_pairs, &mut offsets, &mut csr_edges, &mut in_degrees);

        let mut index_buf = vec![0u32; n];
        let mut low_buf = vec![0u32; n];
        let mut on_stack = vec![0u64; n.div_ceil(64)];
        let mut t_stack = vec![0u32; n];
        let mut c_node = vec![0u32; n];
        let mut c_edge = vec![0u32; n];
        let mut comp_out = vec![0u32; n];
        let mut scratch = TarjanScratch {
            index: &mut index_buf,
            lowlink: &mut low_buf,
            on_stack_bits: &mut on_stack,
            tarjan_stack: &mut t_stack,
            call_stack_node: &mut c_node,
            call_stack_edge: &mut c_edge,
            comp_out: &mut comp_out,
        };
        let scc = tarjan_scc_with_scratch(&csr, &mut scratch);

        let mut pr_cur = vec![0.0f64; n];
        let mut pr_next = vec![0.0f64; n];
        pagerank_in_place(&csr, 0.85, 25, &mut pr_cur, &mut pr_next);

        let mut ranked: Vec<(usize, f64)> = pr_cur.iter().copied().enumerate().collect();
        ranked.sort_unstable_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| pages[a.0].rel_path.cmp(&pages[b.0].rel_path))
        });
        let top_hubs = ranked
            .into_iter()
            .take(5)
            .map(|(idx, score)| HubScore {
                path: pages[idx].rel_path.clone(),
                pagerank: (score * 10_000.0).round() / 10_000.0,
                in_degree: csr.in_degrees[idx],
            })
            .collect();

        GraphMetrics {
            node_count: n,
            edge_count: csr.edges.len(),
            scc_count: scc.scc_count,
            cyclic_sccs: scc.cyclic_sccs,
            largest_scc: scc.largest_scc,
            top_hubs,
        }
    } else {
        GraphMetrics::default()
    };

    // Flag duplicate stems across subdirectories (e.g. concepts/Shared.md & entities/Shared.md)
    for (stem_lower, indices) in &by_stem {
        if indices.len() > 1 {
            let paths: Vec<&str> = indices.iter().map(|&i| pages[i].rel_path.as_str()).collect();
            let joined = paths.join(", ");
            for &idx in indices {
                findings.push(LintFinding {
                    kind: "duplicate_title".to_string(),
                    severity: "warning".to_string(),
                    path: pages[idx].rel_path.clone(),
                    line: None,
                    target: Some(stem_lower.clone()),
                    message: format!("ambiguous note name shared across: {joined}"),
                });
            }
        }
    }

    for (flag, page) in has_inbound.iter_mut().zip(&pages) {
        if !*flag.get_mut() && !META_HUB_PAGES.contains(&page.rel_path.as_str()) {
            findings.push(LintFinding {
                kind: "orphan_page".to_string(),
                severity: "warning".to_string(),
                path: page.rel_path.clone(),
                line: None,
                target: None,
                message: "note has no inbound links from other wiki pages".to_string(),
            });
        }
    }

    findings.sort_unstable();
    findings.dedup();

    let mut findings_by_kind = BTreeMap::new();
    for f in &findings {
        *findings_by_kind.entry(f.kind.clone()).or_insert(0) += 1;
    }

    Ok(LintReport {
        vault_root: vault_root.display().to_string(),
        page_count: pages.len(),
        issue_count: findings.len(),
        findings_by_kind,
        graph_metrics,
        findings,
    })
}

struct LintContext<'a> {
    pages: &'a [LoadedPage],
    allowlist: &'a LintAllowlist,
    by_wiki_rel: &'a FxHashMap<String, usize>,
    by_stem: &'a FxHashMap<String, Vec<usize>>,
    by_alias: &'a FxHashMap<String, Vec<usize>>,
    vault_root: &'a Path,
    wiki_dir: &'a Path,
    has_inbound: &'a [AtomicBool],
}

fn mark_inbound(flags: &[AtomicBool], target_idx: usize) {
    if !flags[target_idx].load(Ordering::Relaxed) {
        flags[target_idx].store(true, Ordering::Relaxed);
    }
}

fn lint_single_page(
    page_idx: usize,
    page: &LoadedPage,
    ctx: &LintContext<'_>,
) -> (Vec<LintFinding>, Vec<(u32, u32)>) {
    let mut local = Vec::new();
    let mut edges =
        Vec::with_capacity(page.parsed.wikilinks.len() + page.parsed.markdown_links.len());
    let src = page_idx as u32;

    if !page.parsed.frontmatter.present {
        local.push(LintFinding {
            kind: "missing_frontmatter".to_string(),
            severity: "warning".to_string(),
            path: page.rel_path.clone(),
            line: Some(1),
            target: None,
            message: "note is missing YAML frontmatter".to_string(),
        });
    }

    for heading in &page.parsed.headings {
        if heading.empty {
            local.push(LintFinding {
                kind: "empty_section".to_string(),
                severity: "warning".to_string(),
                path: page.rel_path.clone(),
                line: Some(heading.line),
                target: Some(heading.text.clone()),
                message: format!("heading '{}' has no section content", heading.text),
            });
        }
    }

    for link in &page.parsed.wikilinks {
        let target_trimmed = link.target.trim();
        // Same-page fragment link [[#heading]] or [[#^block]]
        if target_trimmed.is_empty() {
            if let Some(ref frag) = link.fragment {
                if !fragment_exists(&page.parsed, frag) {
                    local.push(LintFinding {
                        kind: "broken_fragment".to_string(),
                        severity: "error".to_string(),
                        path: page.rel_path.clone(),
                        line: Some(link.line),
                        target: Some(link.raw.clone()),
                        message: format!("missing local anchor '#{frag}'"),
                    });
                }
            }
            continue;
        }

        if is_allowlisted(ctx.allowlist, target_trimmed) {
            continue;
        }

        match resolve_wikilink_target(
            target_trimmed,
            ctx.by_wiki_rel,
            ctx.by_stem,
            ctx.by_alias,
            ctx.vault_root,
        ) {
            LinkResolution::FoundPage(target_idx) => {
                if target_idx != page_idx {
                    mark_inbound(ctx.has_inbound, target_idx);
                    edges.push((src, target_idx as u32));
                }
                if let Some(ref frag) = link.fragment {
                    if !fragment_exists(&ctx.pages[target_idx].parsed, frag) {
                        local.push(LintFinding {
                            kind: "broken_fragment".to_string(),
                            severity: "error".to_string(),
                            path: page.rel_path.clone(),
                            line: Some(link.line),
                            target: Some(link.raw.clone()),
                            message: format!(
                                "missing anchor '#{frag}' in {}",
                                ctx.pages[target_idx].rel_path
                            ),
                        });
                    }
                }
            }
            LinkResolution::FoundAsset => {}
            LinkResolution::Ambiguous(candidates) => {
                for &cand in &candidates {
                    mark_inbound(ctx.has_inbound, cand);
                    if cand != page_idx {
                        edges.push((src, cand as u32));
                    }
                }
                let names: Vec<&str> =
                    candidates.iter().map(|&i| ctx.pages[i].rel_path.as_str()).collect();
                local.push(LintFinding {
                    kind: "ambiguous_wikilink".to_string(),
                    severity: "warning".to_string(),
                    path: page.rel_path.clone(),
                    line: Some(link.line),
                    target: Some(target_trimmed.to_string()),
                    message: format!(
                        "wikilink '[[{target_trimmed}]]' matches multiple notes: {}",
                        names.join(", ")
                    ),
                });
            }
            LinkResolution::Missing => {
                local.push(LintFinding {
                    kind: "broken_wikilink".to_string(),
                    severity: "error".to_string(),
                    path: page.rel_path.clone(),
                    line: Some(link.line),
                    target: Some(target_trimmed.to_string()),
                    message: format!("dangling wikilink '[[{target_trimmed}]]'"),
                });
            }
        }
    }

    if !page.parsed.markdown_links.is_empty() {
        let page_dir = ctx
            .vault_root
            .join(&page.rel_path)
            .parent()
            .map_or_else(|| ctx.wiki_dir.to_path_buf(), Path::to_path_buf);
        for md_link in &page.parsed.markdown_links {
            let clean = md_link.target.split('#').next().unwrap_or("").trim();
            if clean.is_empty() {
                continue;
            }
            let rel_to_page = page_dir.join(clean);
            let rel_to_wiki = ctx.wiki_dir.join(clean);
            let rel_to_vault = ctx.vault_root.join(clean);
            let resolved_md = if rel_to_page.exists() {
                Some(rel_to_page)
            } else if rel_to_wiki.exists() {
                Some(rel_to_wiki)
            } else if rel_to_vault.exists() {
                Some(rel_to_vault)
            } else {
                None
            };

            if let Some(target_path) = resolved_md {
                let posix = to_posix_rel(ctx.vault_root, &target_path);
                let wiki_key = to_lowercase_cow(strip_wiki_prefix_and_ext(&posix));
                if let Some(&target_idx) = ctx.by_wiki_rel.get(wiki_key.as_ref()) {
                    if target_idx != page_idx {
                        mark_inbound(ctx.has_inbound, target_idx);
                        edges.push((src, target_idx as u32));
                    }
                }
            } else {
                local.push(LintFinding {
                    kind: "broken_markdown_link".to_string(),
                    severity: "error".to_string(),
                    path: page.rel_path.clone(),
                    line: Some(md_link.line),
                    target: Some(md_link.target.clone()),
                    message: format!("broken local link '({})'", md_link.target),
                });
            }
        }
    }

    (local, edges)
}

enum LinkResolution {
    FoundPage(usize),
    FoundAsset,
    Ambiguous(Vec<usize>),
    Missing,
}

fn strip_wiki_prefix_and_ext(s: &str) -> &str {
    let without_prefix = s.strip_prefix("wiki/").unwrap_or(s);
    without_prefix.strip_suffix(".md").unwrap_or(without_prefix)
}

fn to_lowercase_cow(s: &str) -> Cow<'_, str> {
    if s.bytes().all(|b| b.is_ascii() && !b.is_ascii_uppercase()) {
        Cow::Borrowed(s)
    } else {
        Cow::Owned(s.to_lowercase())
    }
}

fn resolve_wikilink_target(
    raw_target: &str,
    by_wiki_rel: &FxHashMap<String, usize>,
    by_stem: &FxHashMap<String, Vec<usize>>,
    by_alias: &FxHashMap<String, Vec<usize>>,
    vault_root: &Path,
) -> LinkResolution {
    let trimmed = raw_target.trim_start_matches('/');
    let normalized = to_lowercase_cow(strip_wiki_prefix_and_ext(trimmed));

    if let Some(&idx) = by_wiki_rel.get(normalized.as_ref()) {
        return LinkResolution::FoundPage(idx);
    }

    if !normalized.contains('/') {
        if let Some(matches) = by_stem.get(normalized.as_ref()) {
            return match matches.len() {
                1 => LinkResolution::FoundPage(matches[0]),
                _ => LinkResolution::Ambiguous(matches.clone()),
            };
        }
        if let Some(matches) = by_alias.get(normalized.as_ref()) {
            if matches.len() == 1 {
                return LinkResolution::FoundPage(matches[0]);
            }
            let mut vec = matches.clone();
            vec.sort_unstable();
            vec.dedup();
            return match vec.len() {
                1 => LinkResolution::FoundPage(vec[0]),
                _ => LinkResolution::Ambiguous(vec),
            };
        }
    }

    let asset_wiki = vault_root.join("wiki").join(raw_target);
    let asset_root = vault_root.join(raw_target);
    if asset_wiki.is_file() || asset_root.is_file() {
        return LinkResolution::FoundAsset;
    }

    LinkResolution::Missing
}

fn fragment_exists(parsed: &ParsedNote, fragment: &str) -> bool {
    let trimmed = fragment.trim();
    if let Some(block) = trimmed.strip_prefix('^') {
        return parsed.block_ids.iter().any(|b| b == block);
    }
    if trimmed.is_ascii() {
        return parsed.headings.iter().any(|h| {
            if h.text.is_ascii() {
                h.text.eq_ignore_ascii_case(trimmed)
            } else {
                h.text.to_lowercase() == trimmed.to_lowercase()
            }
        });
    }
    let needle = trimmed.to_lowercase();
    parsed.headings.iter().any(|h| h.text.to_lowercase() == needle)
}

fn load_allowlist(vault_root: &Path) -> LintAllowlist {
    let path = vault_root.join(".vault-meta/lint-allowlist.json");
    let Ok(raw) = fs::read_to_string(path) else {
        return LintAllowlist::default();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

fn is_allowlisted(allowlist: &LintAllowlist, target: &str) -> bool {
    allowlist.dangling_links.iter().any(|pattern| {
        if let Some(prefix) = pattern.strip_suffix('*') {
            target.starts_with(prefix)
        } else {
            target == pattern
        }
    })
}

/// Formats a `LintReport` as clean Markdown.
#[must_use]
pub fn render_markdown(report: &LintReport) -> String {
    let mut out = String::with_capacity(256 + report.findings.len() * 96);
    out.push_str("# Vault Lint Report\n\n");
    let _ = writeln!(out, "- Vault: `{}`", report.vault_root);
    let _ = writeln!(out, "- Pages scanned: {}", report.page_count);
    let _ = writeln!(out, "- Issues found: {}\n", report.issue_count);

    if report.findings.is_empty() {
        out.push_str("No structural issues found.\n");
        return out;
    }

    out.push_str("## Findings\n\n");
    for f in &report.findings {
        let severity = f.severity.to_uppercase();
        if let Some(line) = f.line {
            let _ =
                writeln!(out, "- **[{severity}] `{}`** ({}:{line}): {}", f.kind, f.path, f.message);
        } else {
            let _ = writeln!(out, "- **[{severity}] `{}`** ({}): {}", f.kind, f.path, f.message);
        }
    }
    out
}
