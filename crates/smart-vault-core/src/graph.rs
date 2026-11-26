//! Zero-allocation Compressed Sparse Row (CSR) graph engine:
//! counting-sort construction, bitset BFS, iterative Tarjan SCC, and ping-pong PageRank.

use crate::asm::{ctz_u64, popcnt_u64};

/// Immutable view over a CSR directed graph.
#[derive(Debug, Clone, Copy)]
pub struct CsrGraph<'a> {
    pub node_count: usize,
    pub offsets: &'a [u32],    // len >= node_count + 1
    pub edges: &'a [u32],      // len >= offsets[node_count]
    pub in_degrees: &'a [u32], // len >= node_count
}

/// Summary produced by iterative Tarjan strongly-connected-components analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SccSummary {
    pub scc_count: usize,
    pub cyclic_sccs: usize,
    pub largest_scc: usize,
}

/// Builds a CSR representation of `edge_pairs` (`(u, v)`) in `O(V + E)` without heap allocation.
///
///Caller buffers must satisfy:
/// - `offsets.len() >= node_count + 1`
/// - `edges.len() >= edge_pairs.len()`
/// - `in_degrees.len() >= node_count`
pub fn build_csr_in_place<'a>(
    node_count: usize,
    edge_pairs: &[(u32, u32)],
    offsets: &'a mut [u32],
    edges: &'a mut [u32],
    in_degrees: &'a mut [u32],
) -> CsrGraph<'a> {
    assert!(offsets.len() > node_count, "offsets slice shorter than node_count + 1");
    assert!(in_degrees.len() >= node_count, "in_degrees slice shorter than node_count");

    let offsets = &mut offsets[..=node_count];
    let in_degrees = &mut in_degrees[..node_count];
    offsets.fill(0);
    in_degrees.fill(0);

    for &(u, v) in edge_pairs {
        let ui = u as usize;
        let vi = v as usize;
        if ui < node_count && vi < node_count {
            offsets[ui + 1] += 1;
            in_degrees[vi] += 1;
        }
    }

    for i in 1..=node_count {
        offsets[i] += offsets[i - 1];
    }
    let total_valid = offsets[node_count] as usize;
    assert!(edges.len() >= total_valid, "edges slice shorter than valid edge count");

    // Temporarily advance `offsets[ui]` as a write cursor, then restore via a 1-slot right shift.
    for &(u, v) in edge_pairs {
        let ui = u as usize;
        let vi = v as usize;
        if ui < node_count && vi < node_count {
            let pos = offsets[ui] as usize;
            edges[pos] = v;
            offsets[ui] += 1;
        }
    }

    let mut i = node_count;
    while i > 0 {
        offsets[i] = offsets[i - 1];
        i -= 1;
    }
    offsets[0] = 0;

    CsrGraph { node_count, offsets, edges: &edges[..total_valid], in_degrees }
}

/// Counts nodes with zero incoming links excluding any node whose bit is set in `ignore_bits`.
#[must_use]
pub fn count_orphans(graph: &CsrGraph<'_>, ignore_bits: &[u64]) -> usize {
    let mut orphans = 0usize;
    for (idx, &deg) in graph.in_degrees.iter().enumerate() {
        if deg == 0 {
            let word = idx >> 6;
            let bit = 1u64 << (idx & 63);
            let ignored = word < ignore_bits.len() && (ignore_bits[word] & bit) != 0;
            if !ignored {
                orphans += 1;
            }
        }
    }
    orphans
}

/// Performs zero-allocation bitset BFS from `start`, returning total reachable nodes.
pub fn bfs_reachable_bitset(
    graph: &CsrGraph<'_>,
    start: u32,
    visited_bits: &mut [u64],
    queue: &mut [u32],
) -> usize {
    for w in visited_bits.iter_mut() {
        *w = 0;
    }
    let s = start as usize;
    if s >= graph.node_count || queue.is_empty() {
        return 0;
    }
    set_bit(visited_bits, s);
    queue[0] = start;
    let mut head = 0usize;
    let mut tail = 1usize;

    while head < tail {
        let u = queue[head] as usize;
        head += 1;
        let lo = graph.offsets[u] as usize;
        let hi = graph.offsets[u + 1] as usize;
        for &v in &graph.edges[lo..hi] {
            let vi = v as usize;
            if vi < graph.node_count && !get_bit(visited_bits, vi) && tail < queue.len() {
                set_bit(visited_bits, vi);
                queue[tail] = v;
                tail += 1;
            }
        }
    }

    let mut total = 0usize;
    for &word in visited_bits.iter() {
        total += popcnt_u64(word) as usize;
    }
    total
}

/// Visits every set bit in `bitset` in ascending index order using hardware `ctz_u64`.
pub fn for_each_set_bit(bitset: &[u64], mut f: impl FnMut(usize)) {
    for (word_idx, &word_val) in bitset.iter().enumerate() {
        let mut w = word_val;
        while w != 0 {
            let tz = ctz_u64(w) as usize;
            f((word_idx << 6) + tz);
            w &= w - 1;
        }
    }
}

/// Caller-owned reusable scratch buffers for iterative Tarjan SCC analysis.
pub struct TarjanScratch<'a> {
    pub index: &'a mut [u32],
    pub lowlink: &'a mut [u32],
    pub on_stack_bits: &'a mut [u64],
    pub tarjan_stack: &'a mut [u32],
    pub call_stack_node: &'a mut [u32],
    pub call_stack_edge: &'a mut [u32],
    pub comp_out: &'a mut [u32],
}

/// Iterative, zero-recursion, zero-allocation Tarjan Strongly Connected Components
/// using caller-provided [`TarjanScratch`].
pub fn tarjan_scc_with_scratch(
    graph: &CsrGraph<'_>,
    scratch: &mut TarjanScratch<'_>,
) -> SccSummary {
    const UNVISITED: u32 = u32::MAX;
    let n = graph.node_count;
    assert!(scratch.index.len() >= n);
    assert!(scratch.lowlink.len() >= n);
    assert!(scratch.on_stack_bits.len() >= n.div_ceil(64));
    assert!(scratch.tarjan_stack.len() >= n);
    assert!(scratch.call_stack_node.len() >= n);
    assert!(scratch.call_stack_edge.len() >= n);
    assert!(scratch.comp_out.len() >= n);

    scratch.index[..n].fill(UNVISITED);
    scratch.lowlink[..n].fill(UNVISITED);
    scratch.comp_out[..n].fill(UNVISITED);
    scratch.on_stack_bits.fill(0);

    let mut next_index: u32 = 0;
    let mut t_top: usize = 0;
    let mut scc_count: usize = 0;
    let mut cyclic_sccs: usize = 0;
    let mut largest_scc: usize = 0;

    for root in 0..n {
        if scratch.index[root] != UNVISITED {
            continue;
        }

        let mut c_top: usize = 0;
        scratch.index[root] = next_index;
        scratch.lowlink[root] = next_index;
        next_index += 1;
        set_bit(scratch.on_stack_bits, root);
        scratch.tarjan_stack[t_top] = root as u32;
        t_top += 1;

        scratch.call_stack_node[c_top] = root as u32;
        scratch.call_stack_edge[c_top] = graph.offsets[root];
        c_top += 1;

        while c_top > 0 {
            let frame = c_top - 1;
            let u = scratch.call_stack_node[frame] as usize;
            let edge_idx = scratch.call_stack_edge[frame] as usize;
            let end_idx = graph.offsets[u + 1] as usize;

            if edge_idx < end_idx {
                scratch.call_stack_edge[frame] = (edge_idx + 1) as u32;
                let v = graph.edges[edge_idx] as usize;
                if v >= n {
                    continue;
                }
                if scratch.index[v] == UNVISITED {
                    scratch.index[v] = next_index;
                    scratch.lowlink[v] = next_index;
                    next_index += 1;
                    set_bit(scratch.on_stack_bits, v);
                    scratch.tarjan_stack[t_top] = v as u32;
                    t_top += 1;

                    scratch.call_stack_node[c_top] = v as u32;
                    scratch.call_stack_edge[c_top] = graph.offsets[v];
                    c_top += 1;
                } else if get_bit(scratch.on_stack_bits, v) && scratch.index[v] < scratch.lowlink[u]
                {
                    scratch.lowlink[u] = scratch.index[v];
                }
            } else {
                c_top -= 1;
                if c_top > 0 {
                    let parent = scratch.call_stack_node[c_top - 1] as usize;
                    if scratch.lowlink[u] < scratch.lowlink[parent] {
                        scratch.lowlink[parent] = scratch.lowlink[u];
                    }
                }

                if scratch.lowlink[u] == scratch.index[u] {
                    let scc_id = scc_count as u32;
                    let mut size = 0usize;
                    let mut has_self_loop = false;
                    loop {
                        t_top -= 1;
                        let w = scratch.tarjan_stack[t_top] as usize;
                        clear_bit(scratch.on_stack_bits, w);
                        scratch.comp_out[w] = scc_id;
                        size += 1;
                        if size == 1 && w == u {
                            let lo = graph.offsets[u] as usize;
                            let hi = graph.offsets[u + 1] as usize;
                            for &nbr in &graph.edges[lo..hi] {
                                if nbr as usize == u {
                                    has_self_loop = true;
                                    break;
                                }
                            }
                        }
                        if w == u {
                            break;
                        }
                    }
                    scc_count += 1;
                    if size > 1 || has_self_loop {
                        cyclic_sccs += 1;
                    }
                    if size > largest_scc {
                        largest_scc = size;
                    }
                }
            }
        }
    }

    SccSummary { scc_count, cyclic_sccs, largest_scc }
}

/// Convenience wrapper for [`tarjan_scc_with_scratch`] accepting individual slices.
#[allow(clippy::too_many_arguments)]
pub fn tarjan_scc_iterative(
    graph: &CsrGraph<'_>,
    index_buf: &mut [u32],
    lowlink_buf: &mut [u32],
    on_stack_bits: &mut [u64],
    tarjan_stack: &mut [u32],
    call_stack_node: &mut [u32],
    call_stack_edge: &mut [u32],
    comp_out: &mut [u32],
) -> SccSummary {
    let mut scratch = TarjanScratch {
        index: index_buf,
        lowlink: lowlink_buf,
        on_stack_bits,
        tarjan_stack,
        call_stack_node,
        call_stack_edge,
        comp_out,
    };
    tarjan_scc_with_scratch(graph, &mut scratch)
}

/// Runs `iters` iterations of ping-pong PageRank in place (`cur` holds final ranks).
pub fn pagerank_in_place(
    graph: &CsrGraph<'_>,
    damping: f64,
    iters: usize,
    cur: &mut [f64],
    next: &mut [f64],
) {
    let n = graph.node_count;
    if n == 0 {
        return;
    }
    let init = 1.0 / (n as f64);
    for r in cur.iter_mut().take(n) {
        *r = init;
    }
    let base = (1.0 - damping) / (n as f64);

    for _ in 0..iters {
        let mut dangling_sum = 0.0;
        for v in next.iter_mut().take(n) {
            *v = 0.0;
        }
        for u in 0..n {
            let lo = graph.offsets[u] as usize;
            let hi = graph.offsets[u + 1] as usize;
            let deg = hi - lo;
            if deg == 0 {
                dangling_sum += cur[u];
            } else {
                let share = damping * cur[u] / (deg as f64);
                for &v in &graph.edges[lo..hi] {
                    let vi = v as usize;
                    if vi < n {
                        next[vi] += share;
                    }
                }
            }
        }
        let dangling_contrib = damping * dangling_sum / (n as f64);
        let add = base + dangling_contrib;
        for i in 0..n {
            cur[i] = next[i] + add;
        }
    }
}

#[inline]
fn get_bit(bits: &[u64], idx: usize) -> bool {
    let w = idx >> 6;
    let mask = 1u64 << (idx & 63);
    w < bits.len() && (bits[w] & mask) != 0
}

#[inline]
fn set_bit(bits: &mut [u64], idx: usize) {
    let w = idx >> 6;
    if w < bits.len() {
        bits[w] |= 1u64 << (idx & 63);
    }
}

#[inline]
fn clear_bit(bits: &mut [u64], idx: usize) {
    let w = idx >> 6;
    if w < bits.len() {
        bits[w] &= !(1u64 << (idx & 63));
    }
}
