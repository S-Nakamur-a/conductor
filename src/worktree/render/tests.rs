//! [super::list::truncate_to_width] のテスト。

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
    // CJK文字は1文字2桁幅。3文字で6桁。
    assert_eq!(truncate_to_width("日本語", 10), "日本語");
}

#[test]
fn truncate_multibyte_over_limit() {
    // "日本語テスト" は12桁幅、上限6桁なら "日本語..." になる。
    assert_eq!(truncate_to_width("日本語テスト", 6), "日本語...");
}

#[test]
fn truncate_multibyte_boundary() {
    // 上限5桁: "日"(2) + "本"(2) = 4、次の "語"(2) を足すと5を超える。
    assert_eq!(truncate_to_width("日本語", 5), "日本...");
}

#[test]
fn truncate_empty_string() {
    assert_eq!(truncate_to_width("", 10), "");
}

#[test]
fn truncate_mixed_ascii_and_multibyte() {
    // "a日b" は 1 + 2 + 1 = 4桁幅。
    assert_eq!(truncate_to_width("a日b本c", 4), "a日b...");
}
