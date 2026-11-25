use std::path::Path;

#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use criterion::{criterion_group, criterion_main, Criterion};
use tempfile::TempDir;

use smart_vault::bm25::{build_index, query_index};
use smart_vault::lint::lint_vault;
use smart_vault::vault::init_or_adopt;

fn create_synthetic_vault(page_count: usize) -> TempDir {
    let dir = TempDir::new().expect("tempdir");
    init_or_adopt(dir.path(), "standard", false, true, None).expect("init");
    let concepts = dir.path().join("wiki/concepts");
    for i in 0..page_count {
        let next = (i + 1) % page_count;
        let content = format!(
            "---\ntitle: Concept {i}\naddress: c-{i:06}\ntype: concept\nstatus: evergreen\ntags: [bench, rust]\n---\n\n# Concept {i}\n\nLocal-first Obsidian knowledge node {i} linked to [[Concept {next}]] with sovereign MEM|8 wave memory and BM25 retrieval.\n"
        );
        std::fs::write(concepts.join(format!("Concept {i}.md")), content).expect("write note");
    }
    dir
}

fn bench_vault_ops(c: &mut Criterion) {
    let vault = create_synthetic_vault(200);
    let root: &Path = vault.path();

    c.bench_function("lint_200_pages", |b| {
        b.iter(|| lint_vault(root).expect("lint"));
    });

    c.bench_function("bm25_index_200_pages", |b| {
        b.iter(|| build_index(root).expect("index"));
    });

    build_index(root).expect("seed index");
    c.bench_function("bm25_query_200_pages", |b| {
        b.iter(|| query_index(root, "sovereign wave memory retrieval", 8, false).expect("query"));
    });
}

criterion_group!(benches, bench_vault_ops);
criterion_main!(benches);
