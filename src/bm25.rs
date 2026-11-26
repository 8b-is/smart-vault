use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::{Instant, UNIX_EPOCH};

use chrono::Utc;
use memchr::memchr2;
use rayon::prelude::*;
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use smart_vault_core::asm::simd_ascii_lowercase_in_place;
use smart_vault_core::bm25_math::{idf, term_score};
use unicode_normalization::{char::is_combining_mark, UnicodeNormalization};
use walkdir::WalkDir;

use crate::error::{MemVaultError, Result};
use crate::markdown::parse_frontmatter;
use crate::paths::{assert_unaliased_directory, safe_vault_join, to_posix_rel};
use crate::transaction::{atomic_write_bytes, sha256_bytes, MutationLock};

pub const INDEX_SCHEMA: &str = "smart-vault.bm25.v1";
const K1: f64 = 1.5;
const B: f64 = 0.75;

const STOPWORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "but", "by", "for", "from", "had", "has", "have",
    "he", "her", "his", "if", "in", "into", "is", "it", "its", "of", "on", "or", "she", "that",
    "the", "their", "them", "then", "there", "these", "they", "this", "to", "was", "were", "will",
    "with",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexedDoc {
    pub path: String,
    pub title: String,
    pub address: Option<String>,
    pub sha256: String,
    pub mtime_secs: u64,
    pub byte_len: u64,
    pub dl: usize,
    pub snippet: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Posting {
    pub doc_id: usize,
    pub tf: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VocabEntry {
    pub df: usize,
    pub postings: Vec<Posting>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bm25Index {
    pub schema: String,
    pub k1: f64,
    pub b: f64,
    pub built_at: String,
    pub doc_count: usize,
    pub avg_dl: f64,
    pub docs: Vec<IndexedDoc>,
    pub vocab: BTreeMap<String, VocabEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct IndexBuildStats {
    pub vault_root: String,
    pub index_path: String,
    pub doc_count: usize,
    pub vocab_size: usize,
    pub avg_dl: f64,
    pub elapsed_us: u128,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchHit {
    pub rank: usize,
    pub path: String,
    pub title: String,
    pub address: Option<String>,
    pub score: f64,
    pub stale: bool,
    pub snippet: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchResponse {
    pub query: String,
    pub top_k: usize,
    pub stale_skipped: usize,
    pub elapsed_us: u128,
    pub hits: Vec<SearchHit>,
}

#[inline]
fn increment_tf(map: &mut FxHashMap<String, u32>, tok: &str) {
    if let Some(count) = map.get_mut(tok) {
        *count += 1;
    } else {
        map.insert(tok.to_owned(), 1);
    }
}

#[inline]
fn is_stopword(s: &str) -> bool {
    s.len() <= 5 && STOPWORDS.binary_search(&s).is_ok()
}

fn flush_ascii<F: FnMut(&str)>(buf: &mut String, emit: &mut F) {
    if buf.is_empty() {
        return;
    }
    if memchr2(b'-', b'_', buf.as_bytes()).is_some() {
        if !is_stopword(buf) {
            emit(buf);
        }
        for part in buf.split(['-', '_']).filter(|p| !p.is_empty()) {
            if !is_stopword(part) {
                emit(part);
            }
        }
    } else if !is_stopword(buf) {
        emit(buf);
    }
    buf.clear();
}

fn flush_cjk<F: FnMut(&str)>(buf: &mut Vec<char>, scratch: &mut String, emit: &mut F) {
    if buf.is_empty() {
        return;
    }
    for n in 1..=3 {
        if buf.len() >= n {
            for window in buf.windows(n) {
                scratch.clear();
                for &c in window {
                    scratch.push(c);
                }
                emit(scratch);
            }
        }
    }
    buf.clear();
}

fn tokenize_each_with_bufs<F: FnMut(&str)>(
    text: &str,
    ascii_buf: &mut String,
    cjk_buf: &mut Vec<char>,
    cjk_scratch: &mut String,
    mut emit: F,
) {
    if text.is_ascii() {
        let mut stack_buf = [0u8; 1024];
        for chunk in text.as_bytes().chunks(stack_buf.len()) {
            let slice = &mut stack_buf[..chunk.len()];
            slice.copy_from_slice(chunk);
            simd_ascii_lowercase_in_place(slice);
            for &b in slice.iter() {
                if b.is_ascii_alphanumeric() || ((b == b'-' || b == b'_') && !ascii_buf.is_empty())
                {
                    ascii_buf.push(b as char);
                } else {
                    flush_ascii(ascii_buf, &mut emit);
                }
            }
        }
        flush_ascii(ascii_buf, &mut emit);
        return;
    }

    for ch in text.nfd() {
        if is_combining_mark(ch) {
            continue;
        }
        if ch.is_ascii_alphanumeric() {
            flush_cjk(cjk_buf, cjk_scratch, &mut emit);
            ascii_buf.push(ch.to_ascii_lowercase());
        } else if (ch == '-' || ch == '_') && !ascii_buf.is_empty() {
            ascii_buf.push(ch);
        } else if is_cjk_char(ch) {
            flush_ascii(ascii_buf, &mut emit);
            cjk_buf.push(ch);
        } else {
            flush_ascii(ascii_buf, &mut emit);
            flush_cjk(cjk_buf, cjk_scratch, &mut emit);
        }
    }
    flush_ascii(ascii_buf, &mut emit);
    flush_cjk(cjk_buf, cjk_scratch, &mut emit);
}

/// Tokenizes text with Unicode NFD diacritic folding, ASCII stopword filtering,
/// hyphenated ID preservation (`c-100001`), and CJK 1/2/3-grams.
#[must_use]
pub fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::with_capacity(text.len() / 5 + 8);
    let mut ascii_buf = String::with_capacity(32);
    let mut cjk_buf: Vec<char> = Vec::with_capacity(16);
    let mut cjk_scratch = String::with_capacity(12);

    tokenize_each_with_bufs(text, &mut ascii_buf, &mut cjk_buf, &mut cjk_scratch, |tok| {
        tokens.push(tok.to_owned());
    });
    tokens
}

fn is_cjk_char(c: char) -> bool {
    matches!(
        c as u32,
        0x3040..=0x30FF // Hiragana + Katakana
            | 0x31F0..=0x31FF
            | 0x3400..=0x4DBF // CJK Ext A
            | 0x4E00..=0x9FFF // CJK Unified
            | 0xAC00..=0xD7AF // Hangul
            | 0xF900..=0xFAFF
    )
}

fn build_snippet(body: &str, max_chars: usize) -> String {
    let mut snippet = String::with_capacity(max_chars.min(body.len()));
    let mut char_count = 0usize;
    let mut need_space = false;
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if need_space {
            if char_count >= max_chars {
                break;
            }
            snippet.push(' ');
            char_count += 1;
        }
        for ch in trimmed.chars() {
            if char_count >= max_chars {
                return snippet;
            }
            snippet.push(ch);
            char_count += 1;
        }
        need_space = true;
    }
    snippet
}

/// Builds and persists `.vault-meta/bm25/index.json` across `wiki/**/*.md` using `rayon`.
pub fn build_index(vault_root: &Path) -> Result<IndexBuildStats> {
    let start = Instant::now();
    let wiki_dir = vault_root.join("wiki");
    if !wiki_dir.exists() {
        return Err(MemVaultError::VaultNotFound(format!(
            "missing wiki directory under {}",
            vault_root.display()
        )));
    }
    assert_unaliased_directory(&wiki_dir)?;

    let mut files = Vec::new();
    for entry in WalkDir::new(&wiki_dir).follow_links(false).into_iter().flatten() {
        if entry.file_type().is_symlink() {
            continue;
        }
        if entry.file_type().is_file()
            && entry.path().extension().and_then(|e| e.to_str()) == Some("md")
        {
            files.push(entry.path().to_path_buf());
        }
    }
    files.sort();

    struct RawDoc {
        meta: IndexedDoc,
        tf: FxHashMap<String, u32>,
    }

    let parsed_docs: Vec<RawDoc> = files
        .par_iter()
        .filter_map(|path| {
            let text = fs::read_to_string(path).ok()?;
            let md_meta = fs::metadata(path).ok()?;
            let mtime_secs = md_meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs());
            let byte_len = md_meta.len();
            let sha256 = sha256_bytes(text.as_bytes());
            let fm = parse_frontmatter(&text);
            let rel = to_posix_rel(vault_root, path);
            let title = fm.title.unwrap_or_else(|| {
                path.file_stem().map_or_else(|| rel.clone(), |s| s.to_string_lossy().into_owned())
            });
            let body = &text[fm.body_start_offset..];
            let snippet = build_snippet(body, 200);

            // Weight title + address + tags higher by tokenizing them before body (zero combined String alloc)
            let mut dl = 0usize;
            let est_terms = (body.len() / 10).clamp(16, 4096);
            let mut tf: FxHashMap<String, u32> =
                FxHashMap::with_capacity_and_hasher(est_terms, Default::default());
            let mut ascii_buf = String::with_capacity(32);
            let mut cjk_buf: Vec<char> = Vec::with_capacity(16);
            let mut cjk_scratch = String::with_capacity(12);

            let mut add_piece = |piece: &str| {
                tokenize_each_with_bufs(
                    piece,
                    &mut ascii_buf,
                    &mut cjk_buf,
                    &mut cjk_scratch,
                    |tok| {
                        dl += 1;
                        increment_tf(&mut tf, tok);
                    },
                );
            };

            add_piece(&title);
            add_piece(&title);
            if let Some(ref addr) = fm.address {
                add_piece(addr);
                add_piece(addr);
            }
            for tag in &fm.tags {
                add_piece(tag);
            }
            add_piece(body);

            Some(RawDoc {
                meta: IndexedDoc {
                    path: rel,
                    title,
                    address: fm.address,
                    sha256,
                    mtime_secs,
                    byte_len,
                    dl,
                    snippet,
                },
                tf,
            })
        })
        .collect();

    let doc_count = parsed_docs.len();
    let total_dl: usize = parsed_docs.iter().map(|d| d.meta.dl).sum();
    let avg_dl = if doc_count == 0 { 0.0 } else { total_dl as f64 / doc_count as f64 };

    let mut docs = Vec::with_capacity(doc_count);
    let mut vocab_map: FxHashMap<String, VocabEntry> =
        FxHashMap::with_capacity_and_hasher(doc_count * 16 + 32, Default::default());

    for (doc_id, raw) in parsed_docs.into_iter().enumerate() {
        docs.push(raw.meta);
        for (term, freq) in raw.tf {
            let entry =
                vocab_map.entry(term).or_insert_with(|| VocabEntry { df: 0, postings: Vec::new() });
            entry.df += 1;
            entry.postings.push(Posting { doc_id, tf: freq });
        }
    }
    let vocab: BTreeMap<String, VocabEntry> = vocab_map.into_iter().collect();

    let index = Bm25Index {
        schema: INDEX_SCHEMA.to_string(),
        k1: K1,
        b: B,
        built_at: Utc::now().to_rfc3339(),
        doc_count,
        avg_dl,
        docs,
        vocab,
    };

    let index_rel = Path::new(".vault-meta/bm25/index.json");
    let index_abs = safe_vault_join(vault_root, index_rel)?;
    let _lock = MutationLock::acquire(vault_root)?;
    let encoded = serde_json::to_vec(&index)?;
    atomic_write_bytes(&index_abs, &encoded)?;

    Ok(IndexBuildStats {
        vault_root: vault_root.display().to_string(),
        index_path: to_posix_rel(vault_root, &index_abs),
        doc_count: index.doc_count,
        vocab_size: index.vocab.len(),
        avg_dl: index.avg_dl,
        elapsed_us: start.elapsed().as_micros(),
    })
}

/// Loads the persisted BM25 index from `.vault-meta/bm25/index.json`.
pub fn load_index(vault_root: &Path) -> Result<Bm25Index> {
    let index_abs = safe_vault_join(vault_root, Path::new(".vault-meta/bm25/index.json"))?;
    if !index_abs.is_file() {
        return Err(MemVaultError::InvalidManifest(
            "BM25 index not found; run `smart-vault index` first".to_string(),
        ));
    }
    let raw = fs::read(&index_abs)?;
    Ok(serde_json::from_slice(&raw)?)
}

/// Queries the BM25 index (auto-building if missing and `auto_build == true`) and filters stale pages.
pub fn query_index(
    vault_root: &Path,
    query: &str,
    top_k: usize,
    auto_build: bool,
) -> Result<SearchResponse> {
    let start = Instant::now();
    let index = match load_index(vault_root) {
        Ok(idx) => idx,
        Err(_) if auto_build => {
            build_index(vault_root)?;
            load_index(vault_root)?
        }
        Err(err) => return Err(err),
    };

    Ok(query_loaded_index(vault_root, &index, query, top_k, start))
}

fn query_loaded_index(
    vault_root: &Path,
    index: &Bm25Index,
    query: &str,
    top_k: usize,
    start: Instant,
) -> SearchResponse {
    if index.doc_count == 0 {
        return SearchResponse {
            query: query.to_string(),
            top_k,
            stale_skipped: 0,
            elapsed_us: start.elapsed().as_micros(),
            hits: Vec::new(),
        };
    }

    let mut unique_q: FxHashMap<String, u32> =
        FxHashMap::with_capacity_and_hasher(8, Default::default());
    let mut ascii_buf = String::with_capacity(32);
    let mut cjk_buf: Vec<char> = Vec::with_capacity(16);
    let mut cjk_scratch = String::with_capacity(12);
    tokenize_each_with_bufs(query, &mut ascii_buf, &mut cjk_buf, &mut cjk_scratch, |tok| {
        increment_tf(&mut unique_q, tok);
    });

    if unique_q.is_empty() {
        return SearchResponse {
            query: query.to_string(),
            top_k,
            stale_skipped: 0,
            elapsed_us: start.elapsed().as_micros(),
            hits: Vec::new(),
        };
    }

    let mut matched_terms = Vec::with_capacity(unique_q.len());
    let mut est_docs = 0usize;
    for (term, qtf) in &unique_q {
        if let Some(entry) = index.vocab.get(term) {
            est_docs += entry.postings.len();
            matched_terms.push((entry, *qtf));
        }
    }

    if matched_terms.is_empty() {
        return SearchResponse {
            query: query.to_string(),
            top_k,
            stale_skipped: 0,
            elapsed_us: start.elapsed().as_micros(),
            hits: Vec::new(),
        };
    }

    let avg_dl = index.avg_dl.max(1.0);
    let mut ranked: Vec<(usize, f64)> = if index.doc_count <= 8192
        || est_docs.saturating_mul(2) >= index.doc_count
    {
        let mut dense = vec![0.0f64; index.doc_count];
        for (entry, qtf) in matched_terms {
            let idf_val = idf(index.doc_count, entry.df);
            let q_weight = f64::from(qtf).min(3.0);

            for posting in &entry.postings {
                let doc = &index.docs[posting.doc_id];
                let tf = f64::from(posting.tf);
                let dl = doc.dl as f64;
                let bm25 = term_score(idf_val, tf, dl, avg_dl, index.k1, index.b);
                dense[posting.doc_id] += bm25 * q_weight;
            }
        }
        let mut out = Vec::with_capacity(est_docs.min(index.doc_count));
        for (doc_id, score) in dense.into_iter().enumerate() {
            if score > 0.0 {
                out.push((doc_id, score));
            }
        }
        out
    } else {
        let mut scores: FxHashMap<usize, f64> =
            FxHashMap::with_capacity_and_hasher(est_docs.min(index.doc_count), Default::default());
        for (entry, qtf) in matched_terms {
            let idf_val = idf(index.doc_count, entry.df);
            let q_weight = f64::from(qtf).min(3.0);

            for posting in &entry.postings {
                let doc = &index.docs[posting.doc_id];
                let tf = f64::from(posting.tf);
                let dl = doc.dl as f64;
                let bm25 = term_score(idf_val, tf, dl, avg_dl, index.k1, index.b);
                *scores.entry(posting.doc_id).or_insert(0.0) += bm25 * q_weight;
            }
        }
        scores.into_iter().collect()
    };

    let cmp = |a: &(usize, f64), b: &(usize, f64)| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| index.docs[a.0].path.cmp(&index.docs[b.0].path))
    };

    let pre_sorted = if top_k > 0 && ranked.len() > top_k.saturating_mul(8) {
        let cutoff = top_k.saturating_mul(4).clamp(32, ranked.len());
        if cutoff < ranked.len() {
            ranked.select_nth_unstable_by(cutoff - 1, cmp);
            ranked[..cutoff].sort_unstable_by(cmp);
            cutoff
        } else {
            ranked.sort_unstable_by(cmp);
            ranked.len()
        }
    } else {
        ranked.sort_unstable_by(cmp);
        ranked.len()
    };

    let mut hits = Vec::with_capacity(top_k.min(ranked.len()));
    let mut stale_skipped = 0usize;
    let mut tail_sorted = false;
    for i in 0..ranked.len() {
        if i == pre_sorted && !tail_sorted && pre_sorted < ranked.len() {
            ranked[pre_sorted..].sort_unstable_by(cmp);
            tail_sorted = true;
        }
        let (doc_id, score) = ranked[i];
        let doc = &index.docs[doc_id];
        let is_stale = match safe_vault_join(vault_root, Path::new(&doc.path)) {
            Ok(live_path) => match fs::read(&live_path) {
                Ok(bytes) => sha256_bytes(&bytes) != doc.sha256,
                Err(_) => true,
            },
            Err(_) => true,
        };
        if is_stale {
            stale_skipped += 1;
            continue;
        }
        hits.push(SearchHit {
            rank: hits.len() + 1,
            path: doc.path.clone(),
            title: doc.title.clone(),
            address: doc.address.clone(),
            score: (score * 10_000.0).round() / 10_000.0,
            stale: false,
            snippet: doc.snippet.clone(),
        });
        if hits.len() >= top_k {
            break;
        }
    }

    SearchResponse {
        query: query.to_string(),
        top_k,
        stale_skipped,
        elapsed_us: start.elapsed().as_micros(),
        hits,
    }
}

#[derive(Debug, Deserialize)]
pub struct QueryFixtureFile {
    pub schema: String,
    pub title: String,
    pub queries: Vec<QueryCase>,
}

#[derive(Debug, Deserialize)]
pub struct QueryCase {
    pub id: String,
    pub query: String,
    pub expected_pages: Vec<String>,
    pub category: String,
}

#[derive(Debug, Serialize)]
pub struct BenchCaseResult {
    pub id: String,
    pub category: String,
    pub query: String,
    pub expected_pages: Vec<String>,
    pub returned_pages: Vec<String>,
    pub hit_at_k: bool,
    pub reciprocal_rank: f64,
    pub stale_skipped: usize,
}

#[derive(Debug, Serialize)]
pub struct BenchReport {
    pub total_queries: usize,
    pub hit_count: usize,
    pub mrr: f64,
    pub elapsed_us: u128,
    pub cases: Vec<BenchCaseResult>,
}

/// Evaluates offline retrieval quality against a `queries.json` fixture without mutating the vault.
pub fn benchmark_queries(
    vault_root: &Path,
    queries_file: &Path,
    top_k: usize,
) -> Result<BenchReport> {
    let start = Instant::now();
    let raw = fs::read_to_string(queries_file)?;
    let fixture: QueryFixtureFile = serde_json::from_str(&raw)?;

    let index = load_index(vault_root)?;
    let mut cases = Vec::with_capacity(fixture.queries.len());
    let mut hit_count = 0usize;
    let mut rr_sum = 0.0f64;

    for q in fixture.queries {
        let resp = query_loaded_index(vault_root, &index, &q.query, top_k, Instant::now());
        let returned: Vec<String> = resp.hits.into_iter().map(|h| h.path).collect();

        let (hit, rr) = if q.expected_pages.is_empty() {
            let ok = returned.is_empty();
            (ok, if ok { 1.0 } else { 0.0 })
        } else {
            let first_match = returned
                .iter()
                .position(|p| q.expected_pages.contains(p))
                .map(|idx| 1.0 / (idx as f64 + 1.0));
            (first_match.is_some(), first_match.unwrap_or(0.0))
        };

        if hit {
            hit_count += 1;
        }
        rr_sum += rr;

        cases.push(BenchCaseResult {
            id: q.id,
            category: q.category,
            query: q.query,
            expected_pages: q.expected_pages,
            returned_pages: returned,
            hit_at_k: hit,
            reciprocal_rank: (rr * 10_000.0).round() / 10_000.0,
            stale_skipped: resp.stale_skipped,
        });
    }

    let total = cases.len().max(1);
    Ok(BenchReport {
        total_queries: cases.len(),
        hit_count,
        mrr: ((rr_sum / total as f64) * 10_000.0).round() / 10_000.0,
        elapsed_us: start.elapsed().as_micros(),
        cases,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stopwords_sorted_and_bounded() {
        assert!(STOPWORDS.windows(2).all(|w| w[0] < w[1]));
        assert!(STOPWORDS.iter().all(|w| w.len() <= 5));
        assert!(is_stopword("the"));
        assert!(!is_stopword("obsidian"));
    }

    #[test]
    fn tokenize_preserves_ids_nfd_and_cjk() {
        let tokens = tokenize("The c-100001 Café 全文検索");
        assert_eq!(
            tokens,
            vec![
                "c-100001",
                "c",
                "100001",
                "cafe",
                "全",
                "文",
                "検",
                "索",
                "全文",
                "文検",
                "検索",
                "全文検",
                "文検索"
            ]
        );
    }

    #[test]
    fn build_snippet_truncates_without_headings() {
        let s = build_snippet("# Title\n\nhello\n\n## Sub\nworld", 8);
        assert_eq!(s, "hello wo");
    }

    #[test]
    fn query_rejects_path_traversal_index_entry() {
        let dir = tempfile::TempDir::new().unwrap();
        let meta = dir.path().join(".vault-meta/bm25");
        fs::create_dir_all(&meta).unwrap();
        let fake_index = Bm25Index {
            schema: INDEX_SCHEMA.to_string(),
            k1: K1,
            b: B,
            built_at: "2026-11-26T00:00:00Z".to_string(),
            doc_count: 1,
            avg_dl: 3.0,
            docs: vec![IndexedDoc {
                path: "../../etc/passwd".to_string(),
                title: "Escape".to_string(),
                address: None,
                sha256: "0".repeat(64),
                mtime_secs: 0,
                byte_len: 1,
                dl: 3,
                snippet: "escape".to_string(),
            }],
            vocab: BTreeMap::from([(
                "escape".to_string(),
                VocabEntry { df: 1, postings: vec![Posting { doc_id: 0, tf: 1 }] },
            )]),
        };
        fs::write(meta.join("index.json"), serde_json::to_vec(&fake_index).unwrap()).unwrap();
        let resp = query_index(dir.path(), "escape", 5, false).unwrap();
        assert_eq!(resp.stale_skipped, 1);
        assert!(resp.hits.is_empty());
    }
}
