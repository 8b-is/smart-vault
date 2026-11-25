//! Zero-allocation streaming Obsidian Markdown scanners.

use crate::asm::simd_contains_byte;

/// Zero-copy borrowed view of an Obsidian `[[wikilink]]` or `![[embed]]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WikilinkSpan<'a> {
    pub raw: &'a str,
    pub target: &'a str,
    pub fragment: Option<&'a str>,
    pub alias: Option<&'a str>,
    pub embed: bool,
}

/// Streams every `[[wikilink]]` on `line` to `f` without heap allocation.
#[inline]
pub fn for_each_wikilink<'a>(line: &'a str, mut f: impl FnMut(WikilinkSpan<'a>)) {
    let bytes = line.as_bytes();
    if !simd_contains_byte(bytes, b'[') {
        return;
    }

    let mut pos = 0usize;
    while let Some(rel_open) = find_two_bytes(&bytes[pos..], b'[', b'[') {
        let abs_start = pos + rel_open;
        let embed = abs_start > 0 && bytes[abs_start - 1] == b'!';
        let content_start = abs_start + 2;
        let Some(rel_close) = find_two_bytes(&bytes[content_start..], b']', b']') else {
            break;
        };
        let content_end = content_start + rel_close;
        let inner = &line[content_start..content_end];
        if !inner.is_empty() {
            let (before_alias, alias) = match split_alias_zero_alloc(inner) {
                Some((t, a)) => (t, Some(a)),
                None => (inner, None),
            };
            let (target, fragment) = match before_alias.split_once('#') {
                Some((t, frag)) => (t.trim(), Some(frag.trim())),
                None => (before_alias.trim(), None),
            };
            f(WikilinkSpan { raw: inner, target, fragment, alias, embed });
        }
        pos = content_end + 2;
    }
}

/// Zero-alloc byte-range locator for YAML frontmatter (`--- ... ---` or `...`).
/// Returns `Some((yaml_start, yaml_end, body_start))`.
#[must_use]
pub fn frontmatter_byte_range(input: &[u8]) -> Option<(usize, usize, usize)> {
    let (first_end, next_start) = next_line_bounds(input, 0)?;
    if trim_ascii_ws(&input[..first_end]) != b"---" {
        return None;
    }

    let yaml_start = next_start;
    let mut cursor = yaml_start;
    while cursor < input.len() {
        let (line_end, line_next) = next_line_bounds(input, cursor)?;
        let trimmed = trim_ascii_ws(&input[cursor..line_end]);
        if trimmed == b"---" || trimmed == b"..." {
            return Some((yaml_start, cursor, line_next));
        }
        if line_next == cursor {
            break;
        }
        cursor = line_next;
    }
    None
}

#[inline]
fn find_two_bytes(bytes: &[u8], b0: u8, b1: u8) -> Option<usize> {
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b0 && bytes[i + 1] == b1 {
            return Some(i);
        }
        i += 1;
    }
    None
}

#[inline]
fn split_alias_zero_alloc(inner: &str) -> Option<(&str, &str)> {
    if let Some(pos) = inner.find("\\|") {
        return Some((&inner[..pos], &inner[pos + 2..]));
    }
    inner.split_once('|')
}

#[inline]
fn next_line_bounds(bytes: &[u8], start: usize) -> Option<(usize, usize)> {
    if start > bytes.len() {
        return None;
    }
    let mut i = start;
    while i < bytes.len() && bytes[i] != b'\n' {
        i += 1;
    }
    let line_end = if i > start && bytes[i - 1] == b'\r' { i - 1 } else { i };
    let next_start = if i < bytes.len() { i + 1 } else { bytes.len() };
    Some((line_end, next_start))
}

#[inline]
fn trim_ascii_ws(mut s: &[u8]) -> &[u8] {
    while let Some((&first, rest)) = s.split_first() {
        if first == b' ' || first == b'\t' || first == b'\r' {
            s = rest;
        } else {
            break;
        }
    }
    while let Some((&last, rest)) = s.split_last() {
        if last == b' ' || last == b'\t' || last == b'\r' {
            s = rest;
        } else {
            break;
        }
    }
    s
}
