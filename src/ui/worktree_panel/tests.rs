//! Tests for [`super::list::truncate_to_width`].

use super::list::truncate_to_width;

#[test]
fn truncate_ascii_within_limit() {
    assert_eq!(truncate_to_width("hello", 10), "hello");
}

#[test]
fn truncate_ascii_over_limit() {
    assert_eq!(truncate_to_width("hello world", 5), "hello...");
}

#[test]
fn truncate_multibyte_within_limit() {
    // Each CJK char is 2 columns wide; 3 chars = 6 columns
    assert_eq!(truncate_to_width("日本語", 10), "日本語");
}

#[test]
fn truncate_multibyte_over_limit() {
    // "日本語テスト" = 12 columns; limit to 6 => "日本語..."
    assert_eq!(truncate_to_width("日本語テスト", 6), "日本語...");
}

#[test]
fn truncate_multibyte_boundary() {
    // Limit 5: "日"(2) + "本"(2) = 4, next "語"(2) would exceed 5
    assert_eq!(truncate_to_width("日本語", 5), "日本...");
}

#[test]
fn truncate_empty_string() {
    assert_eq!(truncate_to_width("", 10), "");
}

#[test]
fn truncate_mixed_ascii_and_multibyte() {
    // "a日b" = 1 + 2 + 1 = 4 columns
    assert_eq!(truncate_to_width("a日b本c", 4), "a日b...");
}
