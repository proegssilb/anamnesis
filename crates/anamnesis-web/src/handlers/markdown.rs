//! Markdown for Area/Project/Task descriptions (issue #43): [`strip_html`]
//! on input, [`render`] on output. Also `@`-mention highlighting for
//! descriptions and comments (issue #44), piggybacked on the same render
//! pass since highlighting *is* a markdown-time concern: whether a mention
//! renders highlighted depends on who's viewing, so it can never be baked
//! into the stored source the way the rest of the HTML-to-markdown
//! conversion is.
//!
//! The two save/render steps run in different places for different
//! reasons. [`strip_html`] runs once, at the point a description or comment
//! is saved, so the stored source is always plain markdown — a user who
//! pastes `<b>bold</b>` gets "bold" back, not a live HTML tag sitting in
//! what should be write-once markdown text. [`render`] then runs every time
//! that source is displayed, turning markdown syntax (and mention tokens)
//! into HTML and sanitizing the result — belt-and-suspenders with
//! `strip_html` rather than a substitute for it: `strip_html` keeps the
//! *stored* text free of HTML, `render`'s own sanitize pass keeps the
//! *displayed* HTML safe regardless of how the source reached it (a direct
//! database write, a future import path, ...).
//!
//! A mention is written `@[Display Name](user:USER_ID)` — deliberately not
//! real markdown link syntax resolved through pulldown-cmark's own link
//! handling (a `user:` URL scheme would just get stripped by the sanitizer
//! anyway): [`linkify_mentions`] rewrites each token in the *markdown
//! source*, before parsing, into `**@Display Name**` when `USER_ID` is the
//! viewer or plain `@Display Name` otherwise — ordinary markdown either
//! way, so the rest of the pipeline needs no special cases for it.

use ammonia::Builder;
use minijinja::Value;

/// Tags [`render`] allows in the HTML it produces — the shapes CommonMark
/// itself can generate for a description (paragraphs, emphasis, lists,
/// headings, links, code, blockquotes) and nothing else. Kept separate from
/// [`strip_html`]'s empty allowlist: that one is deliberately total (a
/// stored description should contain no HTML at all), this one is
/// deliberately a safe subset (rendered output is expected to *be* HTML).
const ALLOWED_TAGS: &[&str] = &[
    "p",
    "br",
    "strong",
    "em",
    "del",
    "code",
    "pre",
    "blockquote",
    "ul",
    "ol",
    "li",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "a",
    "hr",
];

/// Strips every HTML tag from `input`, keeping the tags' own text content —
/// `<script>alert(1)</script>` becomes empty (script content is dropped
/// outright, not unwrapped), `<b>bold</b>` becomes `bold`. Run once, on
/// save, so a stored description is always plain markdown source.
pub fn strip_html(input: &str) -> String {
    Builder::empty().clean(input).to_string()
}

/// Renders `input` (already-stripped markdown source) to sanitized HTML,
/// wrapped as a minijinja [`Value`] that templates can interpolate with
/// `{{ }}` and have it come out unescaped — `Value::from_safe_string` is
/// exactly minijinja's marker for "already safe to emit as-is", the same
/// contract as its own `|safe` filter. `viewer` is the current user's id,
/// used only to decide which `@`-mentions (if any) render highlighted —
/// see [`linkify_mentions`].
pub fn render(input: &str, viewer: &str) -> Value {
    let markdown = linkify_mentions(input, viewer);
    let mut unsafe_html = String::new();
    pulldown_cmark::html::push_html(&mut unsafe_html, pulldown_cmark::Parser::new(&markdown));
    let safe_html = Builder::new()
        .tags(ALLOWED_TAGS.iter().copied().collect())
        .link_rel(Some("noopener noreferrer nofollow"))
        .clean(&unsafe_html)
        .to_string();
    Value::from_safe_string(safe_html)
}

/// Rewrites every `@[Display Name](user:USER_ID)` mention token in `source`
/// — bolding it when `USER_ID` equals `viewer` (issue #44: highlight a
/// mention of the current user), or dropping to plain `@Display Name`
/// otherwise ("if the content @-mentions someone else, do nothing" beyond
/// showing the name). A token that never closes (`@[` with no matching `]`,
/// or no matching `)`) is left exactly as written rather than eaten or
/// panicking on — plain text a user typed that merely starts with `@[` is
/// not a bug to crash on.
fn linkify_mentions(source: &str, viewer: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut rest = source;
    while let Some(at) = rest.find("@[") {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        match parse_mention_token(rest) {
            Some((name, user_id, remainder)) => {
                if user_id == viewer {
                    out.push_str("**@");
                    out.push_str(name);
                    out.push_str("**");
                } else {
                    out.push('@');
                    out.push_str(name);
                }
                rest = remainder;
            }
            None => {
                // Not a well-formed token (or not a mention at all) --
                // keep the literal "@[" and resume scanning right after it,
                // so a later real "@[" further along is still found.
                out.push_str("@[");
                rest = &rest[2..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Parses one `@[Display Name](user:USER_ID)` token from the start of
/// `input`, returning `(name, user_id, remainder-after-the-token)` — `None`
/// if `input` doesn't start with a complete, well-formed token.
fn parse_mention_token(input: &str) -> Option<(&str, &str, &str)> {
    let after_bracket = input.strip_prefix("@[")?;
    let (name, after_name) = after_bracket.split_once(']')?;
    let id_part = after_name.strip_prefix("(user:")?;
    let (user_id, remainder) = id_part.split_once(')')?;
    Some((name, user_id, remainder))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_html_drops_tags_but_keeps_their_text() {
        assert_eq!(strip_html("<b>bold</b> and *stars*"), "bold and *stars*");
    }

    #[test]
    fn strip_html_drops_script_content_entirely() {
        assert_eq!(
            strip_html("before<script>alert(1)</script>after"),
            "beforeafter"
        );
    }

    #[test]
    fn render_turns_markdown_into_the_expected_html() {
        let html = render(
            "# Title\n\n**bold** and a [link](https://example.com)",
            "viewer",
        )
        .to_string();
        assert!(html.contains("<h1>Title</h1>"));
        assert!(html.contains("<strong>bold</strong>"));
        assert!(html.contains(r#"<a href="https://example.com""#));
        assert!(html.contains("rel="));
    }

    #[test]
    fn render_sanitizes_any_raw_html_that_slipped_through_to_the_source() {
        let html = render("<script>alert(1)</script>still here", "viewer").to_string();
        assert!(!html.contains("<script"));
        assert!(html.contains("still here"));
    }

    #[test]
    fn a_mention_of_the_viewer_renders_highlighted() {
        let html = render("hey @[Alice](user:alice-id), take a look", "alice-id").to_string();
        assert!(html.contains("<strong>@Alice</strong>"));
    }

    #[test]
    fn a_mention_of_someone_else_renders_as_plain_text() {
        let html = render("hey @[Alice](user:alice-id), take a look", "bob-id").to_string();
        assert!(!html.contains("<strong>"));
        assert!(html.contains("@Alice"));
    }

    #[test]
    fn an_unterminated_mention_token_is_left_as_plain_text() {
        let html = render("this is @[not a real mention", "viewer").to_string();
        assert!(html.contains("@[not a real mention"));
    }

    #[test]
    fn text_that_merely_starts_with_at_bracket_is_not_mistaken_for_a_mention() {
        let html = render("@[just some text] not a mention", "viewer").to_string();
        assert!(html.contains("@[just some text] not a mention"));
    }
}
