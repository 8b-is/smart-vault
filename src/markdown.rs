use std::borrow::Cow;
use std::collections::BTreeMap;

use memchr::{memchr, memchr_iter};
use serde::Serialize;
use smart_vault_core::scan::{for_each_wikilink, frontmatter_byte_range};

/// Parsed YAML-subset frontmatter metadata from a Markdown note.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Frontmatter {
    pub present: bool,
    pub fields: BTreeMap<String, String>,
    pub title: Option<String>,
    pub note_type: Option<String>,
    pub status: Option<String>,
    pub address: Option<String>,
    pub tags: Vec<String>,
    pub aliases: Vec<String>,
    pub body_start_offset: usize,
}

/// Parsed wikilink occurrence inside a Markdown document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Wikilink {
    pub raw: String,
    pub target: String,
    pub fragment: Option<String>,
    pub alias: Option<String>,
    pub embed: bool,
    pub line: usize,
}

/// Parsed relative Markdown link `[text](target)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MarkdownLink {
    pub text: String,
    pub target: String,
    pub embed: bool,
    pub line: usize,
}

/// Heading metadata inside a Markdown note.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Heading {
    pub level: usize,
    pub text: String,
    pub line: usize,
    pub empty: bool,
}

/// Complete structural parse of a Markdown note.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ParsedNote {
    pub frontmatter: Frontmatter,
    pub headings: Vec<Heading>,
    pub wikilinks: Vec<Wikilink>,
    pub markdown_links: Vec<MarkdownLink>,
    pub block_ids: Vec<String>,
}

/// Parses frontmatter, headings, wikilinks, markdown links, and block IDs in a single fast pass.
#[must_use]
pub fn parse_markdown(input: &str) -> ParsedNote {
    let frontmatter = parse_frontmatter(input);
    let body = &input[frontmatter.body_start_offset..];
    let body_start_line =
        memchr_iter(b'\n', &input.as_bytes()[..frontmatter.body_start_offset]).count() + 1;

    let mut headings: Vec<Heading> = Vec::new();
    let mut open_headings: Vec<usize> = Vec::new();
    let mut wikilinks = Vec::new();
    let mut markdown_links = Vec::new();
    let mut block_ids = Vec::new();

    let mut fence_char: Option<u8> = None;
    let mut fence_len: usize = 0;

    let mark_content = |headings: &mut [Heading], open: &mut Vec<usize>| {
        for idx in open.drain(..) {
            headings[idx].empty = false;
        }
    };

    for (idx, raw_line) in body.lines().enumerate() {
        let line_no = body_start_line + idx;
        let trimmed = raw_line.trim();

        if let Some(fc) = fence_char {
            // Inside fenced code block: substantive content, check for closing fence
            mark_content(&mut headings, &mut open_headings);
            if is_closing_fence(trimmed, fc, fence_len) {
                fence_char = None;
                fence_len = 0;
            }
            continue;
        }

        if let Some((fc, flen)) = parse_opening_fence(trimmed) {
            fence_char = Some(fc);
            fence_len = flen;
            mark_content(&mut headings, &mut open_headings);
            continue;
        }

        if let Some((level, heading_text)) = parse_atx_heading(trimmed) {
            while let Some(&top) = open_headings.last() {
                if headings[top].level >= level {
                    open_headings.pop();
                } else {
                    break;
                }
            }
            let heading_idx = headings.len();
            headings.push(Heading {
                level,
                text: heading_text.to_string(),
                line: line_no,
                empty: true,
            });
            open_headings.push(heading_idx);
            continue;
        }

        if !trimmed.is_empty() {
            mark_content(&mut headings, &mut open_headings);
        }

        // Check trailing ^block-id outside code blocks
        if let Some(bid) = extract_trailing_block_id(trimmed) {
            block_ids.push(bid.to_string());
        }

        // Fast-path skip inline-code stripping and link scanners when no `[` exists
        if memchr(b'[', raw_line.as_bytes()).is_some() {
            let stripped = strip_inline_code(raw_line);
            extract_wikilinks_from_line(&stripped, line_no, &mut wikilinks);
            extract_md_links_from_line(&stripped, line_no, &mut markdown_links);
        }
    }

    ParsedNote { frontmatter, headings, wikilinks, markdown_links, block_ids }
}

/// Extracts YAML frontmatter using zero-alloc `smart_vault_core::scan::frontmatter_byte_range`.
#[must_use]
pub fn parse_frontmatter(input: &str) -> Frontmatter {
    let Some((fm_start, fm_end, body_start_offset)) = frontmatter_byte_range(input.as_bytes())
    else {
        return Frontmatter::default();
    };

    let mut fm = Frontmatter { present: true, body_start_offset, ..Frontmatter::default() };

    let mut current_list_key: Option<&str> = None;
    for raw_line in input[fm_start..fm_end].lines() {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        if (raw_line.starts_with("  -") || raw_line.starts_with("- ")) && trimmed.starts_with('-') {
            let item = unquote_yaml(trimmed.trim_start_matches('-'));
            if let Some(key) = current_list_key {
                if !item.is_empty() {
                    if key == "tags" {
                        fm.tags.push(item.to_string());
                    } else if key == "aliases" {
                        fm.aliases.push(item.to_string());
                    }
                }
            }
            continue;
        }

        current_list_key = None;
        if let Some((k, v)) = trimmed.split_once(':') {
            let key = k.trim();
            let val_raw = v.trim();
            if val_raw.is_empty() {
                current_list_key = Some(key);
                continue;
            }
            if val_raw.starts_with('[') && val_raw.ends_with(']') {
                let inner = &val_raw[1..val_raw.len() - 1];
                let items: Vec<String> = inner
                    .split(',')
                    .map(unquote_yaml)
                    .filter(|s| !s.is_empty())
                    .map(ToString::to_string)
                    .collect();
                fm.fields.insert(key.to_string(), items.join(", "));
                if key == "tags" {
                    fm.tags.extend(items);
                } else if key == "aliases" {
                    fm.aliases.extend(items);
                }
                continue;
            }

            let val = unquote_yaml(val_raw);
            match key {
                "title" => fm.title = Some(val.to_string()),
                "type" => fm.note_type = Some(val.to_string()),
                "status" => fm.status = Some(val.to_string()),
                "address" => fm.address = Some(val.to_string()),
                "tags" => {
                    for part in val.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                        fm.tags.push(part.to_string());
                    }
                }
                "aliases" => {
                    for part in val.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                        fm.aliases.push(part.to_string());
                    }
                }
                _ => {}
            }
            fm.fields.insert(key.to_string(), val.to_string());
        }
    }

    fm
}

fn unquote_yaml(s: &str) -> &str {
    let t = s.trim();
    if t.len() >= 2
        && ((t.starts_with('"') && t.ends_with('"')) || (t.starts_with('\'') && t.ends_with('\'')))
    {
        &t[1..t.len() - 1]
    } else {
        t
    }
}

fn parse_opening_fence(trimmed: &str) -> Option<(u8, usize)> {
    let first = *trimmed.as_bytes().first()?;
    if first != b'`' && first != b'~' {
        return None;
    }
    let count = trimmed.bytes().take_while(|&b| b == first).count();
    if count >= 3 {
        if first == b'`' && trimmed[count..].contains('`') {
            return None;
        }
        Some((first, count))
    } else {
        None
    }
}

fn is_closing_fence(trimmed: &str, fence_char: u8, min_len: usize) -> bool {
    let count = trimmed.bytes().take_while(|&b| b == fence_char).count();
    count >= min_len && trimmed[count..].trim().is_empty()
}

fn parse_atx_heading(trimmed: &str) -> Option<(usize, &str)> {
    let level = trimmed.bytes().take_while(|&b| b == b'#').count();
    if (1..=6).contains(&level) {
        let rest = &trimmed[level..];
        if rest.is_empty() || rest.starts_with(' ') || rest.starts_with('\t') {
            let text = rest.trim().trim_end_matches('#').trim();
            if !text.is_empty() {
                return Some((level, text));
            }
        }
    }
    None
}

fn extract_trailing_block_id(trimmed: &str) -> Option<&str> {
    let idx = trimmed.rfind(" ^")?;
    let id = &trimmed[idx + 2..];
    if !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
        Some(id)
    } else {
        None
    }
}

fn strip_inline_code(line: &str) -> Cow<'_, str> {
    let bytes = line.as_bytes();
    let Some(first_tick) = memchr(b'`', bytes) else {
        return Cow::Borrowed(line);
    };
    let mut out = String::with_capacity(line.len());
    out.push_str(&line[..first_tick]);
    let mut i = first_tick;
    while i < bytes.len() {
        if bytes[i] == b'`' {
            let ticks = bytes[i..].iter().take_while(|&&b| b == b'`').count();
            let mut j = i + ticks;
            let mut closed = false;
            while j + ticks <= bytes.len() {
                let Some(rel) = memchr(b'`', &bytes[j..bytes.len() - ticks + 1]) else {
                    break;
                };
                j += rel;
                if bytes[j..j + ticks].iter().all(|&b| b == b'`')
                    && (j + ticks == bytes.len() || bytes[j + ticks] != b'`')
                {
                    i = j + ticks;
                    closed = true;
                    break;
                }
                let run = bytes[j..].iter().take_while(|&&b| b == b'`').count();
                j += run;
            }
            if !closed {
                out.push_str(&line[i..i + ticks]);
                i += ticks;
            }
        } else if let Some(rel) = memchr(b'`', &bytes[i..]) {
            out.push_str(&line[i..i + rel]);
            i += rel;
        } else {
            out.push_str(&line[i..]);
            break;
        }
    }
    Cow::Owned(out)
}

fn extract_wikilinks_from_line(line: &str, line_no: usize, out: &mut Vec<Wikilink>) {
    for_each_wikilink(line, |span| {
        out.push(Wikilink {
            raw: span.raw.to_string(),
            target: span.target.to_string(),
            fragment: span.fragment.map(str::to_string),
            alias: span.alias.map(str::to_string),
            embed: span.embed,
            line: line_no,
        });
    });
}

fn extract_md_links_from_line(line: &str, line_no: usize, out: &mut Vec<MarkdownLink>) {
    let bytes = line.as_bytes();
    let mut i = 0;
    while let Some(rel) = memchr(b'[', &bytes[i..]) {
        i += rel;
        // Skip wikilinks [[...]]
        if i + 1 < bytes.len() && bytes[i + 1] == b'[' {
            i += 2;
            continue;
        }
        if i > 0 && bytes[i - 1] == b'[' {
            i += 1;
            continue;
        }
        let embed = i > 0 && bytes[i - 1] == b'!';
        if let Some(close_bracket) = memchr(b']', &bytes[i + 1..]) {
            let text = &line[i + 1..i + 1 + close_bracket];
            let after_bracket = i + 1 + close_bracket + 1;
            if after_bracket < bytes.len() && bytes[after_bracket] == b'(' {
                if let Some(close_paren) = memchr(b')', &bytes[after_bracket + 1..]) {
                    let raw_target =
                        line[after_bracket + 1..after_bracket + 1 + close_paren].trim();
                    let target =
                        raw_target.split_whitespace().next().unwrap_or("").trim_matches(['<', '>']);
                    if !target.is_empty() && !is_external_uri(target) {
                        out.push(MarkdownLink {
                            text: text.to_string(),
                            target: target.to_string(),
                            embed,
                            line: line_no,
                        });
                    }
                    i = after_bracket + 1 + close_paren + 1;
                    continue;
                }
            }
        }
        i += 1;
    }
}

fn starts_with_ascii_ci(s: &str, prefix: &str) -> bool {
    s.get(..prefix.len()).is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

#[must_use]
pub fn is_external_uri(target: &str) -> bool {
    target.starts_with('#')
        || starts_with_ascii_ci(target, "http://")
        || starts_with_ascii_ci(target, "https://")
        || starts_with_ascii_ci(target, "mailto:")
        || starts_with_ascii_ci(target, "obsidian://")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_inline_and_fenced_wikilinks() {
        let md = "---\ntitle: Test\ntype: concept\n---\n\n# Test\n\n[[Real]]\n`[[Ghost]]`\n\n```\n[[Fenced]]\n```\n";
        let parsed = parse_markdown(md);
        assert!(parsed.frontmatter.present);
        assert_eq!(parsed.wikilinks.len(), 1);
        assert_eq!(parsed.wikilinks[0].target, "Real");
    }

    #[test]
    fn handles_escaped_pipe_and_fragment() {
        let md = "- [[concepts/Alpha\\|Escaped]]\n- [[Alpha#Known heading]]\n";
        let parsed = parse_markdown(md);
        assert_eq!(parsed.wikilinks.len(), 2);
        assert_eq!(parsed.wikilinks[0].target, "concepts/Alpha");
        assert_eq!(parsed.wikilinks[0].alias.as_deref(), Some("Escaped"));
        assert_eq!(parsed.wikilinks[1].target, "Alpha");
        assert_eq!(parsed.wikilinks[1].fragment.as_deref(), Some("Known heading"));
    }

    #[test]
    fn tracks_nested_empty_sections_and_external_uris() {
        let md = "---\ntitle: \"Quoted\"\ntags: [rust, \"obs\"]\n...\n\n# Top\n\n## Empty\n\n## Parent\n\n### Child\n\nBody with [ext](HTTPS://example.com) and [loc](notes/a.md).\n";
        let parsed = parse_markdown(md);
        assert!(parsed.frontmatter.present);
        assert_eq!(parsed.frontmatter.title.as_deref(), Some("Quoted"));
        assert_eq!(parsed.frontmatter.tags, vec!["rust", "obs"]);
        assert_eq!(parsed.headings.len(), 4);
        assert!(!parsed.headings[0].empty); // Top
        assert!(parsed.headings[1].empty); // Empty
        assert!(!parsed.headings[2].empty); // Parent
        assert!(!parsed.headings[3].empty); // Child
        assert_eq!(parsed.markdown_links.len(), 1);
        assert_eq!(parsed.markdown_links[0].target, "notes/a.md");
    }
}
