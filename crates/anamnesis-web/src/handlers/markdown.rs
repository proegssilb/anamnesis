//! Markdown for Area/Project/Task descriptions (issue #43): [`strip_html`]
//! on input, [`render`] on output.
//!
//! The two run in different places for different reasons. [`strip_html`]
//! runs once, at the point a description is saved, so the stored source is
//! always plain markdown — a user who pastes `<b>bold</b>` gets "bold" back,
//! not a live HTML tag sitting in what should be write-once markdown text.
//! [`render`] then runs every time that source is displayed, turning
//! markdown syntax into HTML and sanitizing the result — belt-and-suspenders
//! with `strip_html` rather than a substitute for it: `strip_html` keeps the
//! *stored* text free of HTML, `render`'s own sanitize pass keeps the
//! *displayed* HTML safe regardless of how the source reached it (a direct
//! database write, a future import path, ...).

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
/// contract as its own `|safe` filter.
pub fn render(input: &str) -> Value {
    let mut unsafe_html = String::new();
    pulldown_cmark::html::push_html(&mut unsafe_html, pulldown_cmark::Parser::new(input));
    let safe_html = Builder::new()
        .tags(ALLOWED_TAGS.iter().copied().collect())
        .link_rel(Some("noopener noreferrer nofollow"))
        .clean(&unsafe_html)
        .to_string();
    Value::from_safe_string(safe_html)
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
        let html = render("# Title\n\n**bold** and a [link](https://example.com)").to_string();
        assert!(html.contains("<h1>Title</h1>"));
        assert!(html.contains("<strong>bold</strong>"));
        assert!(html.contains(r#"<a href="https://example.com""#));
        assert!(html.contains("rel="));
    }

    #[test]
    fn render_sanitizes_any_raw_html_that_slipped_through_to_the_source() {
        let html = render("<script>alert(1)</script>still here").to_string();
        assert!(!html.contains("<script"));
        assert!(html.contains("still here"));
    }
}
