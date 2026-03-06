/// Text preparation and sanitisation for embedding input.
use std::borrow::Cow;

/// Approximate chars-per-token ratio for truncation.
const CHARS_PER_TOKEN: usize = 4;
/// Default context window in tokens.
const DEFAULT_MAX_TOKENS: usize = 8192;

/// Default max input length in characters.
pub const DEFAULT_MAX_INPUT_CHARS: usize = DEFAULT_MAX_TOKENS * CHARS_PER_TOKEN;

/// Prepare article text for embedding: concatenate title + content.
pub fn prepare_article_text(title: &str, content: Option<&str>) -> String {
    let raw = match content {
        Some(c) if !c.is_empty() => format!("{title}\n\n{c}"),
        _ => title.to_string(),
    };
    sanitize(&raw)
}

/// Truncate text to `max_chars`, returning the input unchanged (zero-copy)
/// when it already fits.
pub fn prepare_input(text: &str, max_chars: usize) -> Cow<'_, str> {
    if text.len() <= max_chars {
        return Cow::Borrowed(text);
    }
    let mut end = max_chars;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    Cow::Owned(text[..end].to_string())
}

/// Strip null bytes and other control characters that can cause issues
/// with embedding APIs. Preserves newlines and tabs.
fn sanitize(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t' || *c == '\r')
        .collect()
}
