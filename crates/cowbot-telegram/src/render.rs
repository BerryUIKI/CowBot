//! The answer as Telegram shows it (ADR-0058).
//!
//! An agent's reply is Markdown, and Telegram's `sendMessage` takes either no
//! markup, MarkdownV2, or HTML. HTML is the one that can be produced correctly
//! without a full parser: the only thing that has to be right is escaping, and
//! escaping wrongly is what turns a reply into a `400`.
//!
//! The renderer is deliberately narrow. It converts the three constructs an
//! answer actually uses — fenced code, `inline code`, and emphasis — and escapes
//! everything else. Anything it does not understand comes out as literal text,
//! which is the only safe direction for a transformation an operator cannot see.

/// Convert an answer to Telegram's HTML.
///
/// Escapes first, then adds tags: doing it the other way round would let a
/// `<b>` inside a code block become markup, and would let an unclosed tag break
/// the whole message.
pub fn to_html(markdown: &str) -> String {
    let mut html = String::with_capacity(markdown.len());
    let mut rest = markdown;

    while let Some(fence) = rest.find("```") {
        html.push_str(&inline(&rest[..fence]));
        let after = &rest[fence + 3..];
        // The info string ("rust", "json") is not part of the code.
        let body_start = after.find('\n').map(|index| index + 1).unwrap_or(0);
        let body = &after[body_start..];
        match body.find("```") {
            Some(end) => {
                html.push_str("<pre><code>");
                html.push_str(&escape(&body[..end]));
                html.push_str("</code></pre>");
                rest = &body[end + 3..];
            }
            None => {
                // An unclosed fence: the rest of the answer is code. Rendering
                // it literally beats dropping it, and an unclosed `<pre>` is
                // something Telegram accepts.
                html.push_str("<pre><code>");
                html.push_str(&escape(&after[body_start..]));
                html.push_str("</code></pre>");
                rest = "";
            }
        }
    }
    html.push_str(&inline(rest));
    html
}

/// One line's worth of non-code text: inline code, then emphasis.
fn inline(text: &str) -> String {
    let mut html = String::with_capacity(text.len());
    let mut rest = text;

    while let Some(open) = rest.find('`') {
        html.push_str(&emphasis(&rest[..open]));
        let after = &rest[open + 1..];
        match after.find('`') {
            Some(end) => {
                html.push_str("<code>");
                html.push_str(&escape(&after[..end]));
                html.push_str("</code>");
                rest = &after[end + 1..];
            }
            None => {
                // A lone backtick is a character, not a span.
                html.push_str(&emphasis(&format!("`{after}")));
                rest = "";
            }
        }
    }
    html.push_str(&emphasis(rest));
    html
}

/// Bold and italic, on the parts that are plain text.
fn emphasis(text: &str) -> String {
    let escaped = escape(text);
    // **bold** first, so the single-asterisk pass cannot eat one of its stars.
    let bold = pair_replace(&escaped, "**", "<b>", "</b>");
    pair_replace(&bold, "*", "<i>", "</i>")
}

/// Wrap each balanced `marker` pair in `open`/`close`, leaving an odd one alone.
fn pair_replace(text: &str, marker: &str, open: &str, close: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(marker) {
        let after = &rest[start + marker.len()..];
        let Some(end) = after.find(marker) else {
            // Unpaired: leave the marker as the character it is.
            result.push_str(rest);
            return result;
        };
        result.push_str(&rest[..start]);
        result.push_str(open);
        result.push_str(&after[..end]);
        result.push_str(close);
        rest = &after[end + marker.len()..];
    }
    result.push_str(rest);
    result
}

/// The three characters Telegram's parser reserves.
///
/// `&` first: escaping it after the others would turn their `&` into `&amp;`.
pub fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            other => escaped.push(other),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_answer_is_escaped_and_left_alone() {
        assert_eq!(to_html("Deploy is green."), "Deploy is green.");
        assert_eq!(to_html("a < b && c > d"), "a &lt; b &amp;&amp; c &gt; d");
    }

    #[test]
    fn ampersands_are_escaped_once_not_twice() {
        // Escaping `&` after `<` would turn the `&` of `&lt;` into `&amp;lt;`.
        assert_eq!(escape("<&>"), "&lt;&amp;&gt;");
    }

    #[test]
    fn a_fenced_block_becomes_preformatted_text_and_keeps_its_characters() {
        let html = to_html("Before\n```rust\nlet x = a < b;\n```\nAfter");
        assert_eq!(
            html,
            "Before\n<pre><code>let x = a &lt; b;\n</code></pre>\nAfter"
        );
    }

    #[test]
    fn markup_inside_a_code_block_stays_literal() {
        // The whole point of escaping first: a `<b>` in code is code.
        let html = to_html("```\n<b>not bold</b>\n```");
        assert_eq!(
            html,
            "<pre><code>&lt;b&gt;not bold&lt;/b&gt;\n</code></pre>"
        );
    }

    #[test]
    fn an_unclosed_fence_renders_the_rest_as_code_rather_than_dropping_it() {
        let html = to_html("here:\n```\nstill typing");
        assert!(html.contains("still typing"));
        assert!(html.starts_with("here:\n<pre><code>"));
    }

    #[test]
    fn inline_code_and_emphasis_are_rendered() {
        assert_eq!(to_html("run `cargo fmt`"), "run <code>cargo fmt</code>");
        assert_eq!(to_html("**done**"), "<b>done</b>");
        assert_eq!(to_html("*maybe*"), "<i>maybe</i>");
        assert_eq!(to_html("**a** and *b*"), "<b>a</b> and <i>b</i>");
    }

    #[test]
    fn a_stray_marker_is_a_character_not_an_unclosed_tag() {
        // An unbalanced marker must not produce markup Telegram will refuse.
        assert_eq!(to_html("2 * 3 = 6"), "2 * 3 = 6");
        assert_eq!(to_html("a `tick"), "a `tick");
    }

    #[test]
    fn a_bold_markers_stars_are_not_reused_as_italic() {
        assert_eq!(to_html("**bold**"), "<b>bold</b>");
    }

    #[test]
    fn an_emoji_survives_the_round_trip() {
        assert_eq!(to_html("🚀 **go**"), "🚀 <b>go</b>");
    }
}
