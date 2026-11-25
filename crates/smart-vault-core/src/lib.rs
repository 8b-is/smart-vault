#![no_std]
//! Zero-allocation `#![no_std]` kernel for `smart-vault`.
//!
//! Provides:
//! - [`asm`]: `aarch64` inline `asm!` / NEON SIMD primitives and constant-time equality
//! - [`graph`]: zero-alloc Compressed Sparse Row (CSR) graph builder, iterative Tarjan SCC,
//!   bitset BFS, and ping-pong PageRank
//! - [`scan`]: zero-alloc streaming wikilink and YAML frontmatter byte-range scanners
//! - [`bm25_math`]: dependency-free Okapi BM25 math and stack-allocated [`bm25_math::TopK`]

pub mod asm;
pub mod bm25_math;
pub mod graph;
pub mod scan;

#[cfg(test)]
mod tests {
    use crate::asm::{
        ct_eq_bytes, ctz_u64, fnv1a_64, popcnt_u64, simd_ascii_lowercase_in_place,
        simd_contains_byte,
    };
    use crate::bm25_math::{idf, ln_f64, term_score, TopK};
    use crate::graph::{
        bfs_reachable_bitset, build_csr_in_place, count_orphans, for_each_set_bit,
        pagerank_in_place, tarjan_scc_iterative,
    };
    use crate::scan::{for_each_wikilink, frontmatter_byte_range};

    #[test]
    fn asm_neon_primitives_and_ct_eq() {
        assert!(ct_eq_bytes(b"sha256-digest-abcd", b"sha256-digest-abcd"));
        assert!(!ct_eq_bytes(b"sha256-digest-abcd", b"sha256-digest-abce"));
        assert!(!ct_eq_bytes(b"short", b"longer"));

        assert_eq!(ctz_u64(0), 64);
        assert_eq!(ctz_u64(1), 0);
        assert_eq!(ctz_u64(0x8000_0000_0000_0000), 63);
        assert_eq!(popcnt_u64(0xF0F0_0F0F), 16);

        let line = b"0123456789abcdef[[Target]]tail";
        assert!(simd_contains_byte(line, b'['));
        assert!(!simd_contains_byte(line, b'!'));

        let mut buf = *b"HELLO_WORLD_0123456789_RUST_NEON!";
        simd_ascii_lowercase_in_place(&mut buf);
        assert_eq!(&buf, b"hello_world_0123456789_rust_neon!");
        assert_ne!(fnv1a_64(b"alpha"), fnv1a_64(b"beta"));
    }

    #[test]
    fn csr_tarjan_scc_bfs_and_pagerank_zero_alloc() {
        // Cycle 0 -> 1 -> 2 -> 0, plus leaf 2 -> 3, plus orphan 4
        let pairs = [(0u32, 1u32), (1, 2), (2, 0), (2, 3)];
        let mut offsets = [0u32; 6];
        let mut edges = [0u32; 4];
        let mut in_deg = [0u32; 5];
        let g = build_csr_in_place(5, &pairs, &mut offsets, &mut edges, &mut in_deg);

        // Node 4 has in_degree 0 (and ignore node 0? no, node 0 has in_deg 1 from 2 -> only 4 is orphan)
        let ignore = [0u64; 1];
        assert_eq!(count_orphans(&g, &ignore), 1);

        let mut visited = [0u64; 1];
        let mut queue = [0u32; 5];
        let reach = bfs_reachable_bitset(&g, 0, &mut visited, &mut queue);
        assert_eq!(reach, 4);

        let mut seen = [false; 5];
        for_each_set_bit(&visited, |idx| seen[idx] = true);
        assert_eq!(seen, [true, true, true, true, false]);

        let mut index_buf = [0u32; 5];
        let mut low_buf = [0u32; 5];
        let mut on_stack = [0u64; 1];
        let mut t_stack = [0u32; 5];
        let mut c_node = [0u32; 5];
        let mut c_edge = [0u32; 5];
        let mut comp_out = [0u32; 5];
        let scc = tarjan_scc_iterative(
            &g,
            &mut index_buf,
            &mut low_buf,
            &mut on_stack,
            &mut t_stack,
            &mut c_node,
            &mut c_edge,
            &mut comp_out,
        );
        assert_eq!(scc.scc_count, 3); // {3}, {0,1,2}, {4}
        assert_eq!(scc.cyclic_sccs, 1);
        assert_eq!(scc.largest_scc, 3);
        assert_eq!(comp_out[0], comp_out[1]);
        assert_eq!(comp_out[1], comp_out[2]);

        let mut cur = [0.0f64; 5];
        let mut next = [0.0f64; 5];
        pagerank_in_place(&g, 0.85, 25, &mut cur, &mut next);
        let sum: f64 = cur.iter().sum();
        assert!((sum - 1.0).abs() < 1e-6);
        assert!(cur[3] > cur[4]);
    }

    #[test]
    fn zero_alloc_scan_and_bm25_topk() {
        let md = b"---\ntitle: Alpha\n---\n\n# Alpha\n";
        let (ys, ye, bs) = frontmatter_byte_range(md).expect("fm");
        assert_eq!(&md[ys..ye], b"title: Alpha\n");
        assert_eq!(&md[bs..], b"\n# Alpha\n");

        let mut count = 0usize;
        for_each_wikilink("See [[concepts/Alpha\\|A]] and ![[Beta#H]]", |span| {
            if count == 0 {
                assert_eq!(span.target, "concepts/Alpha");
                assert_eq!(span.alias, Some("A"));
                assert!(!span.embed);
            } else {
                assert_eq!(span.target, "Beta");
                assert_eq!(span.fragment, Some("H"));
                assert!(span.embed);
            }
            count += 1;
        });
        assert_eq!(count, 2);

        assert!((ln_f64(1.0)).abs() < 1e-12);
        assert!((ln_f64(core::f64::consts::E) - 1.0).abs() < 1e-12);
        let idf_v = idf(100, 5);
        assert!(term_score(idf_v, 3.0, 40.0, 50.0, 1.5, 0.75) > 0.0);

        let mut topk = TopK::<3>::new();
        topk.push(10, 1.2);
        topk.push(20, 3.8);
        topk.push(30, 0.4);
        topk.push(40, 2.5);
        topk.push(50, 9.1);
        assert_eq!(topk.as_slice(), &[(50, 9.1), (20, 3.8), (40, 2.5)]);
    }
}
