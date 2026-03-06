use std::borrow::Cow;

use super::text::{prepare_article_text, prepare_input};

#[test]
fn prepare_article_text_with_content() {
    assert_eq!(
        prepare_article_text("Title", Some("Content")),
        "Title\n\nContent"
    );
}

#[test]
fn prepare_article_text_no_content() {
    assert_eq!(prepare_article_text("Title", None), "Title");
    assert_eq!(prepare_article_text("Title", Some("")), "Title");
}

#[test]
fn prepare_article_text_strips_null_bytes() {
    let text = prepare_article_text("Hello\0World", Some("foo\0bar"));
    assert!(!text.contains('\0'));
    assert_eq!(text, "HelloWorld\n\nfoobar");
}

#[test]
fn prepare_input_no_truncation_borrows() {
    let text = "hello world";
    let result = prepare_input(text, 100);
    assert!(matches!(result, Cow::Borrowed(_)));
    assert_eq!(&*result, "hello world");
}

#[test]
fn prepare_input_truncates() {
    assert_eq!(&*prepare_input("hello world", 5), "hello");
}

#[test]
fn prepare_input_respects_char_boundaries() {
    let text = "héllo"; // é is 2 bytes
    let result = prepare_input(text, 2);
    // Should truncate to "h" (1 byte) rather than splitting the é
    assert_eq!(&*result, "h");
}
