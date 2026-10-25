//! Splits an Obsidian note into embeddable chunks.
//!
//! Rules (see the vault note "Issue 2 - Local Rust CLI and MCP"): split on
//! H1-H3, prefix the embedded text with `Title > Heading > Subheading`, cap
//! chunks at roughly `MAX_CHUNK_TOKENS`, keep fenced code whole, and treat
//! frontmatter, wikilinks, tags, comments and block ids the way Obsidian does.

use std::sync::LazyLock;

use regex::{Captures, Regex};
use sha2::{Digest, Sha256};

pub const MAX_CHUNK_TOKENS: usize = 300;
const MIN_CHUNK_TOKENS: usize = 40;

static HEADING: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(#{1,6})\s+(.+?)\s*$").unwrap());
static COMMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)%%.*?%%").unwrap());
static WIKILINK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(!)?\[\[([^\]|#]*)(#[^\]|]*)?(?:\|([^\]]*))?\]\]").unwrap()
});
static MD_IMAGE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"!\[[^\]]*\]\([^)]*\)").unwrap());
static MD_LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[([^\]]*)\]\([^)]*\)").unwrap());
static BLOCK_ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[ \t]\^[A-Za-z0-9-]+\s*$").unwrap());
static INLINE_TAG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^|\s)#([\p{L}\p{N}_/-]*[\p{L}_/-][\p{L}\p{N}_/-]*)").unwrap()
});

#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    /// Headings above this chunk, without the note title.
    pub heading_path: Vec<String>,
    /// Original markdown, for display.
    pub text: String,
    /// What gets embedded: breadcrumb plus cleaned text.
    pub embed_text: String,
    /// Wikilink targets found in this chunk.
    pub links: Vec<String>,
    /// SHA-256 of `embed_text`; unchanged hash means the embedding can be reused.
    pub hash: String,
}

#[derive(Debug, Clone, Default)]
pub struct ParsedNote {
    pub title: String,
    pub tags: Vec<String>,
    pub aliases: Vec<String>,
    pub links: Vec<String>,
    pub chunks: Vec<Chunk>,
}

/// `title` is the note's filename without extension; it is the note's identity in Obsidian.
pub fn parse_note(title: &str, source: &str) -> ParsedNote {
    let source = source.replace("\r\n", "\n");
    let (frontmatter, body) = split_frontmatter(&source);

    let mut note = ParsedNote {
        title: title.to_string(),
        tags: frontmatter.tags,
        aliases: frontmatter.aliases,
        ..Default::default()
    };

    let elements = elements(body);
    for element in &elements {
        if let Element::Block(block) = element {
            if !block.is_code {
                for tag in inline_tags(&block.raw) {
                    push_unique(&mut note.tags, tag);
                }
            }
            for link in &block.links {
                push_unique(&mut note.links, link.clone());
            }
        }
    }

    let drafts = merge_tiny(sections_to_drafts(elements));
    note.chunks = drafts.into_iter().map(|d| finish(title, d)).collect();
    note
}

#[derive(Default)]
struct Frontmatter {
    tags: Vec<String>,
    aliases: Vec<String>,
}

fn split_frontmatter(source: &str) -> (Frontmatter, &str) {
    let Some(rest) = source.strip_prefix("---\n") else {
        return (Frontmatter::default(), source);
    };
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        let trimmed = line.trim_end();
        if trimmed == "---" || trimmed == "..." {
            let yaml = &rest[..offset];
            let body = &rest[offset + line.len()..];
            return (parse_frontmatter(yaml), body);
        }
        offset += line.len();
    }
    (Frontmatter::default(), source)
}

fn parse_frontmatter(yaml: &str) -> Frontmatter {
    let Ok(serde_yaml_ng::Value::Mapping(map)) = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(yaml)
    else {
        return Frontmatter::default();
    };
    let list = |keys: &[&str]| -> Vec<String> {
        let mut out = Vec::new();
        for key in keys {
            if let Some(value) = map.get(*key) {
                for item in yaml_to_list(value) {
                    push_unique(&mut out, item);
                }
            }
        }
        out
    };
    Frontmatter {
        tags: list(&["tags", "tag"]),
        aliases: list(&["aliases", "alias"]),
    }
}

fn yaml_to_list(value: &serde_yaml_ng::Value) -> Vec<String> {
    use serde_yaml_ng::Value;
    match value {
        Value::String(s) => s
            .split(|c: char| c == ',' || c.is_whitespace())
            .map(|s| s.trim().trim_start_matches('#').to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        Value::Sequence(items) => items
            .iter()
            .filter_map(|v| v.as_str())
            .map(|s| s.trim().trim_start_matches('#').to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

enum Segment {
    Text(String),
    Code(String),
}

/// Separates fenced code blocks from prose so prose rules never touch code.
fn segments(body: &str) -> Vec<Segment> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut fence: Option<(char, usize)> = None;

    for line in body.lines() {
        let indent = line.len() - line.trim_start_matches(' ').len();
        let stripped = &line[indent..];
        let run = fence_run(stripped);

        match fence {
            None => {
                if let (true, Some((ch, len))) = (indent <= 3, run) {
                    if !buf.is_empty() {
                        out.push(Segment::Text(std::mem::take(&mut buf)));
                    }
                    fence = Some((ch, len));
                }
                buf.push_str(line);
                buf.push('\n');
            }
            Some((open_ch, open_len)) => {
                buf.push_str(line);
                buf.push('\n');
                let closes = matches!(run, Some((ch, len)) if ch == open_ch && len >= open_len)
                    && stripped.trim_start_matches(open_ch).trim().is_empty();
                if closes {
                    out.push(Segment::Code(std::mem::take(&mut buf)));
                    fence = None;
                }
            }
        }
    }
    if !buf.is_empty() {
        out.push(match fence {
            Some(_) => Segment::Code(buf),
            None => Segment::Text(buf),
        });
    }
    out
}

fn fence_run(line: &str) -> Option<(char, usize)> {
    let ch = line.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let len = line.chars().take_while(|c| *c == ch).count();
    (len >= 3).then_some((ch, len))
}

struct Block {
    raw: String,
    clean: String,
    links: Vec<String>,
    is_code: bool,
}

enum Element {
    Heading { level: usize, text: String, raw: String },
    Block(Block),
}

fn elements(body: &str) -> Vec<Element> {
    let mut out = Vec::new();
    for segment in segments(body) {
        match segment {
            Segment::Code(code) => {
                let raw = code.trim_end().to_string();
                out.push(Element::Block(Block {
                    clean: raw.clone(),
                    raw,
                    links: Vec::new(),
                    is_code: true,
                }));
            }
            Segment::Text(text) => {
                let text = COMMENT.replace_all(&text, "");
                let mut lines: Vec<&str> = Vec::new();
                for line in text.lines() {
                    if let Some(caps) = HEADING.captures(line) {
                        flush_block(&mut lines, &mut out);
                        out.push(Element::Heading {
                            level: caps[1].len(),
                            text: clean_text(&caps[2]),
                            raw: line.trim_end().to_string(),
                        });
                    } else if line.trim().is_empty() {
                        flush_block(&mut lines, &mut out);
                    } else {
                        lines.push(line);
                    }
                }
                flush_block(&mut lines, &mut out);
            }
        }
    }
    out
}

fn flush_block(lines: &mut Vec<&str>, out: &mut Vec<Element>) {
    if lines.is_empty() {
        return;
    }
    let raw = lines.join("\n");
    lines.clear();
    out.push(Element::Block(Block {
        clean: clean_text(&raw),
        links: extract_links(&raw),
        raw,
        is_code: false,
    }));
}

fn clean_text(raw: &str) -> String {
    let text = WIKILINK.replace_all(raw, |caps: &Captures| {
        if caps.get(1).is_some() {
            return String::new();
        }
        let target = caps[2].trim();
        if let Some(alias) = caps.get(4) {
            return alias.as_str().to_string();
        }
        let heading = caps
            .get(3)
            .map(|m| m.as_str().trim_start_matches('#'))
            .filter(|h| !h.starts_with('^') && !h.is_empty());
        match (target.is_empty(), heading) {
            (_, None) => target.to_string(),
            (true, Some(h)) => h.to_string(),
            (false, Some(h)) => format!("{target} > {h}"),
        }
    });
    let text = MD_IMAGE.replace_all(&text, "");
    let text = MD_LINK.replace_all(&text, "$1");
    text.lines()
        .map(|line| BLOCK_ID.replace(line, "").trim_end().to_string())
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

fn extract_links(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    for caps in WIKILINK.captures_iter(raw) {
        let target = caps[2].trim();
        if !target.is_empty() {
            push_unique(&mut out, target.to_string());
        }
    }
    out
}

fn inline_tags(raw: &str) -> Vec<String> {
    INLINE_TAG
        .captures_iter(raw)
        .map(|caps| caps[1].to_string())
        .collect()
}

fn push_unique(list: &mut Vec<String>, item: String) {
    if !list.contains(&item) {
        list.push(item);
    }
}

fn tokens(s: &str) -> usize {
    s.chars().count().div_ceil(4)
}

struct Draft {
    path: Vec<String>,
    raw: String,
    clean: String,
    links: Vec<String>,
    /// Heading text when this draft opens a section, so merging keeps it visible to the embedder.
    lead: Option<String>,
}

struct Piece {
    raw: String,
    clean: String,
    links: Vec<String>,
    sep: &'static str,
}

fn sections_to_drafts(elements: Vec<Element>) -> Vec<Draft> {
    let mut drafts = Vec::new();
    let mut stack: Vec<(usize, String)> = Vec::new();
    let mut heading_raw = String::new();
    let mut heading_text: Option<String> = None;
    let mut blocks: Vec<Block> = Vec::new();

    let finish_section = |stack: &[(usize, String)],
                          heading_raw: &mut String,
                          heading_text: &mut Option<String>,
                          blocks: &mut Vec<Block>,
                          drafts: &mut Vec<Draft>| {
        let path: Vec<String> = stack.iter().map(|(_, t)| t.clone()).collect();
        let taken = std::mem::take(blocks);
        drafts.extend(pack_section(
            path,
            std::mem::take(heading_raw),
            heading_text.take(),
            taken,
        ));
    };

    for element in elements {
        match element {
            Element::Heading { level, text, raw } if level <= 3 => {
                finish_section(&stack, &mut heading_raw, &mut heading_text, &mut blocks, &mut drafts);
                while stack.last().is_some_and(|(l, _)| *l >= level) {
                    stack.pop();
                }
                stack.push((level, text.clone()));
                heading_raw = raw;
                heading_text = Some(text);
            }
            Element::Heading { raw, text, .. } => blocks.push(Block {
                raw,
                clean: text,
                links: Vec::new(),
                is_code: false,
            }),
            Element::Block(block) => blocks.push(block),
        }
    }
    finish_section(&stack, &mut heading_raw, &mut heading_text, &mut blocks, &mut drafts);
    drafts
}

fn pack_section(
    path: Vec<String>,
    heading_raw: String,
    heading_text: Option<String>,
    blocks: Vec<Block>,
) -> Vec<Draft> {
    let mut pieces = Vec::new();
    for block in blocks {
        if !block.is_code && tokens(&block.clean) > MAX_CHUNK_TOKENS {
            for (i, sentence) in split_sentences(&block.raw).into_iter().enumerate() {
                pieces.push(Piece {
                    clean: clean_text(&sentence),
                    links: extract_links(&sentence),
                    raw: sentence,
                    sep: if i == 0 { "\n\n" } else { " " },
                });
            }
        } else {
            pieces.push(Piece {
                raw: block.raw,
                clean: block.clean,
                links: block.links,
                sep: "\n\n",
            });
        }
    }

    let mut drafts: Vec<Draft> = Vec::new();
    let mut first = true;
    let new_draft = |first: &mut bool| {
        let lead = if *first { heading_text.clone() } else { None };
        let raw = if *first { heading_raw.clone() } else { String::new() };
        *first = false;
        Draft { path: path.clone(), raw, clean: String::new(), links: Vec::new(), lead }
    };

    let mut current = new_draft(&mut first);
    let mut has_body = false;
    for piece in pieces {
        if has_body && tokens(&current.clean) + tokens(&piece.clean) > MAX_CHUNK_TOKENS {
            drafts.push(current);
            current = new_draft(&mut first);
            has_body = false;
        }
        if !current.raw.is_empty() {
            current.raw.push_str(if has_body { piece.sep } else { "\n\n" });
        }
        current.raw.push_str(&piece.raw);
        if !piece.clean.is_empty() {
            if !current.clean.is_empty() {
                current.clean.push_str(if piece.sep == " " { " " } else { "\n\n" });
            }
            current.clean.push_str(&piece.clean);
        }
        for link in piece.links {
            push_unique(&mut current.links, link);
        }
        has_body = true;
    }
    if has_body {
        drafts.push(current);
    }
    drafts
}

fn split_sentences(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        current.push(c);
        let ends = matches!(c, '.' | '!' | '?') && chars.peek().is_some_and(|n| n.is_whitespace());
        if ends || c == '\n' {
            let sentence = current.trim().to_string();
            if !sentence.is_empty() {
                out.push(sentence);
            }
            current.clear();
        }
    }
    let rest = current.trim();
    if !rest.is_empty() {
        out.push(rest.to_string());
    }
    out
}

/// Folds sections too small to stand alone into the chunk before them.
fn merge_tiny(drafts: Vec<Draft>) -> Vec<Draft> {
    let mut out: Vec<Draft> = Vec::new();
    for draft in drafts {
        match out.last_mut() {
            Some(prev)
                if tokens(&draft.clean) < MIN_CHUNK_TOKENS
                    && tokens(&prev.clean) + tokens(&draft.clean) <= MAX_CHUNK_TOKENS =>
            {
                prev.raw.push_str("\n\n");
                prev.raw.push_str(&draft.raw);
                if let Some(lead) = draft.lead {
                    prev.clean.push_str("\n\n");
                    prev.clean.push_str(&lead);
                }
                if !draft.clean.is_empty() {
                    prev.clean.push('\n');
                    prev.clean.push_str(&draft.clean);
                }
                for link in draft.links {
                    push_unique(&mut prev.links, link);
                }
            }
            _ => out.push(draft),
        }
    }
    out
}

fn finish(title: &str, draft: Draft) -> Chunk {
    let mut path = draft.path;
    if path.first().is_some_and(|h| h.trim().eq_ignore_ascii_case(title.trim())) {
        path.remove(0);
    }

    let mut breadcrumb = title.to_string();
    for heading in &path {
        breadcrumb.push_str(" > ");
        breadcrumb.push_str(heading);
    }
    let embed_text = format!("{breadcrumb}\n{}", draft.clean);
    let hash: String = Sha256::digest(embed_text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();

    Chunk {
        heading_path: path,
        text: draft.raw.trim().to_string(),
        embed_text,
        links: draft.links,
        hash,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lorem(words: usize) -> String {
        (0..words).map(|i| format!("word{i}")).collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn frontmatter_is_metadata_not_text() {
        let note = parse_note(
            "Note",
            "---\ntags: [a, b/c]\naliases:\n  - Other Name\n---\n# Note\n\nBody text here.\n",
        );
        assert_eq!(note.tags, vec!["a", "b/c"]);
        assert_eq!(note.aliases, vec!["Other Name"]);
        assert_eq!(note.chunks.len(), 1);
        assert!(!note.chunks[0].embed_text.contains("aliases"));
    }

    #[test]
    fn frontmatter_tags_as_string() {
        let note = parse_note("Note", "---\ntags: one, two\n---\nBody\n");
        assert_eq!(note.tags, vec!["one", "two"]);
    }

    #[test]
    fn breadcrumb_uses_filename_and_skips_duplicate_h1() {
        let body = lorem(60);
        let note = parse_note(
            "My Note",
            &format!("# My Note\n\n{body}\n\n## Setup\n\n### Gotchas\n\n{body}\n"),
        );
        assert_eq!(note.chunks[0].heading_path, Vec::<String>::new());
        let last = note.chunks.last().unwrap();
        assert_eq!(last.heading_path, vec!["Setup", "Gotchas"]);
        assert!(last.embed_text.starts_with("My Note > Setup > Gotchas\n"));
    }

    #[test]
    fn wikilinks_are_rewritten_and_recorded() {
        let note = parse_note(
            "N",
            "See [[Other Note]], [[Other Note|the alias]], [[Other#Heading]], [[#Local]] and ![[Embedded]].\n",
        );
        let chunk = &note.chunks[0];
        assert!(chunk.embed_text.contains("See Other Note, the alias, Other > Heading, Local and ."));
        assert!(chunk.text.contains("[[Other Note]]"));
        assert_eq!(chunk.links, vec!["Other Note", "Other", "Embedded"]);
        assert_eq!(note.links, chunk.links);
    }

    #[test]
    fn comments_and_block_ids_are_stripped() {
        let note = parse_note("N", "visible %%hidden\nacross lines%% text ^abc-1\n");
        let clean = &note.chunks[0].embed_text;
        assert!(clean.contains("visible  text"));
        assert!(!clean.contains("hidden"));
        assert!(!clean.contains("^abc"));
    }

    #[test]
    fn inline_tags_collected_but_not_from_code_or_headings() {
        let note = parse_note("N", "# Heading\n\ntext #real and #nested/tag\n\n```\n#notatag\n```\n");
        assert_eq!(note.tags, vec!["real", "nested/tag"]);
    }

    #[test]
    fn code_fence_is_never_split_or_rewritten() {
        let code = format!("```rust\nlet x = \"%%d [[not a link]]\";\n{}\n```", lorem(400));
        let note = parse_note("N", &format!("## A\n\nbefore\n\n{code}\n\nafter\n"));
        let holder = note.chunks.iter().find(|c| c.text.contains("```rust")).unwrap();
        assert!(holder.text.contains(&code));
        assert!(holder.embed_text.contains("[[not a link]]"));
        assert!(note.links.is_empty());
    }

    #[test]
    fn large_section_is_split_under_the_cap() {
        let paragraphs: Vec<String> = (0..10).map(|_| lorem(60)).collect();
        let note = parse_note("N", &format!("## Big\n\n{}\n", paragraphs.join("\n\n")));
        assert!(note.chunks.len() > 1);
        for chunk in &note.chunks {
            assert!(tokens(&chunk.embed_text) <= MAX_CHUNK_TOKENS + 20, "{}", chunk.embed_text.len());
            assert_eq!(chunk.heading_path, vec!["Big"]);
        }
    }

    #[test]
    fn huge_paragraph_is_split_on_sentences() {
        let sentence = format!("{}.", lorem(40));
        let paragraph = vec![sentence; 20].join(" ");
        let note = parse_note("N", &paragraph);
        assert!(note.chunks.len() > 1);
        for chunk in &note.chunks {
            assert!(tokens(&chunk.embed_text) <= MAX_CHUNK_TOKENS + 20);
        }
    }

    #[test]
    fn tiny_sections_merge_into_previous_chunk() {
        let note = parse_note("N", "## One\n\nfirst bit\n\n## Two\n\nsecond bit\n\n## Three\n\nthird bit\n");
        assert_eq!(note.chunks.len(), 1);
        let text = &note.chunks[0].embed_text;
        assert!(text.contains("first bit") && text.contains("Two") && text.contains("third bit"));
    }

    #[test]
    fn substantial_sections_stay_separate() {
        let body = lorem(60);
        let note = parse_note("N", &format!("## One\n\n{body}\n\n## Two\n\n{body}\n"));
        assert_eq!(note.chunks.len(), 2);
        assert_eq!(note.chunks[1].heading_path, vec!["Two"]);
    }

    #[test]
    fn empty_note_has_no_chunks() {
        assert!(parse_note("N", "").chunks.is_empty());
        assert!(parse_note("N", "---\ntags: [x]\n---\n").chunks.is_empty());
    }

    #[test]
    fn hash_is_stable_and_tracks_content() {
        let a = parse_note("N", "## S\n\nsame text\n");
        let b = parse_note("N", "## S\n\nsame text\n");
        let c = parse_note("N", "## S\n\nchanged text\n");
        assert_eq!(a.chunks[0].hash, b.chunks[0].hash);
        assert_ne!(a.chunks[0].hash, c.chunks[0].hash);
    }

    #[test]
    fn crlf_is_handled() {
        let note = parse_note("N", "---\r\ntags: [x]\r\n---\r\n## S\r\n\r\ntext\r\n");
        assert_eq!(note.tags, vec!["x"]);
        assert_eq!(note.chunks[0].heading_path, vec!["S"]);
    }
}
