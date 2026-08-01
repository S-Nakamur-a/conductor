//! Unit tests for pty_manager: scroll-arrow encoding, paste sanitization,
//! mouse-wheel encoding, raw-history trimming/reflow, UTF-8 chunking, and
//! locale-override detection.

use std::collections::VecDeque;

use super::io::{encode_mouse_wheel, sanitize_pasted_text, scroll_arrow_sequence};
use super::locale::{utf8_chunks, utf8_locale_overrides};
use super::PtyManager;

#[test]
fn arrow_sequence_honors_decckm_and_direction() {
    // Application cursor keys mode → SS3 (ESC O); note the letter O (0x4f).
    assert_eq!(scroll_arrow_sequence(true, true), b"\x1bOA");
    assert_eq!(scroll_arrow_sequence(false, true), b"\x1bOB");
    // Normal mode → CSI (ESC [).
    assert_eq!(scroll_arrow_sequence(true, false), b"\x1b[A");
    assert_eq!(scroll_arrow_sequence(false, false), b"\x1b[B");
}

// ── sanitize_pasted_text ─────────────────────────────────────────────

#[test]
fn sanitize_keeps_plain_text_tabs_and_newlines() {
    let input = "echo hello\tworld\nsecond line\n";
    assert_eq!(sanitize_pasted_text(input), input);
}

#[test]
fn sanitize_normalizes_crlf_and_lone_cr_to_lf() {
    assert_eq!(sanitize_pasted_text("a\r\nb\rc"), "a\nb\nc");
}

#[test]
fn sanitize_strips_csi_escape_sequences() {
    // SGR color codes around the word must be removed, leaving plain text.
    let input = "\x1b[31mred\x1b[0m text";
    assert_eq!(sanitize_pasted_text(input), "red text");
}

#[test]
fn sanitize_removes_embedded_bracketed_paste_end_marker() {
    // The load-bearing security case: a clipboard that smuggles a paste-end
    // marker (`\x1b[201~`) followed by a command must not be able to break
    // out of bracketed paste. The whole CSI sequence (incl. the `~` final
    // byte) is dropped, so no `201~` survives.
    let input = "safe\x1b[201~rm -rf /\n";
    let out = sanitize_pasted_text(input);
    assert_eq!(out, "saferm -rf /\n");
    assert!(!out.contains("201~"));
    assert!(!out.contains('\x1b'));
}

#[test]
fn sanitize_strips_osc_string_sequence() {
    // OSC (window title) terminated by BEL — the whole thing is removed.
    let input = "before\x1b]0;evil title\x07after";
    assert_eq!(sanitize_pasted_text(input), "beforeafter");
}

#[test]
fn sanitize_drops_bare_control_bytes_but_keeps_text() {
    // NUL, BEL, backspace, DEL all dropped; surrounding text intact.
    let input = "a\x00b\x07c\x08d\x7fe";
    assert_eq!(sanitize_pasted_text(input), "abcde");
}

#[test]
fn sanitize_preserves_multibyte_text() {
    let input = "こんにちは\tworld\n";
    assert_eq!(sanitize_pasted_text(input), input);
}

// ── encode_mouse_wheel ───────────────────────────────────────────────

#[test]
fn encode_mouse_wheel_sgr_up_and_down() {
    // SGR: wheel up = button 64, down = 65; coordinates inline, final 'M'.
    assert_eq!(
        encode_mouse_wheel(true, 5, 9, vt100::MouseProtocolEncoding::Sgr),
        b"\x1b[<64;5;9M".to_vec()
    );
    assert_eq!(
        encode_mouse_wheel(false, 1, 1, vt100::MouseProtocolEncoding::Sgr),
        b"\x1b[<65;1;1M".to_vec()
    );
}

#[test]
fn encode_mouse_wheel_x10_offsets_by_32() {
    // Legacy X10: CSI M Cb Cx Cy with each value +32. Up = 64+32 = 96 = '`'.
    let seq = encode_mouse_wheel(true, 2, 3, vt100::MouseProtocolEncoding::Default);
    assert_eq!(seq, vec![0x1b, b'[', b'M', 96, 34, 35]);
}

#[test]
fn encode_mouse_wheel_clamps_coordinates_to_at_least_one() {
    // 0 coordinates are clamped to 1 so the SGR form stays well-formed.
    assert_eq!(
        encode_mouse_wheel(true, 0, 0, vt100::MouseProtocolEncoding::Sgr),
        b"\x1b[<64;1;1M".to_vec()
    );
}

#[test]
fn trim_raw_history_keeps_under_cap_and_aligns_to_line() {
    let mut history: VecDeque<u8> = b"aaaa\nbbbb\ncccc\ndddd\n".iter().copied().collect();
    // Cap below current length forces a trim.
    PtyManager::trim_raw_history(&mut history, 10);
    let remaining: Vec<u8> = history.iter().copied().collect();
    // Must be at or under the cap...
    assert!(remaining.len() <= 10, "len={}", remaining.len());
    // ...and resume at a clean line boundary (no leading partial line).
    let text = String::from_utf8(remaining).unwrap();
    for line in text.split_inclusive('\n') {
        if line.ends_with('\n') {
            // Every retained complete line is one of the originals.
            assert!(["aaaa\n", "bbbb\n", "cccc\n", "dddd\n"].contains(&line));
        }
    }
    // The oldest content ("aaaa") must have been dropped.
    assert!(!text.contains("aaaa"));
}

#[test]
fn trim_raw_history_noop_when_within_cap() {
    let mut history: VecDeque<u8> = b"hello\n".iter().copied().collect();
    PtyManager::trim_raw_history(&mut history, 1024);
    assert_eq!(history.iter().copied().collect::<Vec<u8>>(), b"hello\n");
}

#[test]
fn trim_raw_history_noop_when_cap_equals_len() {
    let mut history: VecDeque<u8> = b"abcd\n".iter().copied().collect();
    let len = history.len();
    PtyManager::trim_raw_history(&mut history, len);
    assert_eq!(history.len(), len);
}

#[test]
fn trim_raw_history_empty_is_safe() {
    let mut history: VecDeque<u8> = VecDeque::new();
    PtyManager::trim_raw_history(&mut history, 0);
    assert!(history.is_empty());
}

/// A buffer over cap with NO newline must NOT be drained to empty — a blank
/// terminal on resize is far worse than a slightly imperfect first line.
/// (This is the failure mode of escape-sequence-heavy TUI output.)
#[test]
fn trim_raw_history_keeps_bytes_when_no_newline() {
    let mut history: VecDeque<u8> = std::iter::repeat_n(b'x', 100).collect();
    PtyManager::trim_raw_history(&mut history, 10);
    assert!(!history.is_empty(), "history was drained to empty");
    assert!(history.len() <= 10);
    assert!(history.iter().all(|&b| b == b'x'));
}

/// Replaying the raw byte stream into a fresh parser at a new width must
/// reflow content that originally wrapped at the old width — this is the
/// core of `resize_session`'s column-change path. (vt100's own `set_size`
/// does not reflow, which is the bug this whole change fixes.)
#[test]
fn replay_reflows_to_new_width() {
    // A single 12-char logical line, no explicit newline.
    let stream = b"ABCDEFGHIJKL";

    // Narrow parser (cols=4): the line wraps across 3 physical rows, but
    // vt100 tracks the wrap so it is still one logical line.
    let mut narrow = vt100::Parser::new(10, 4, 100);
    narrow.process(stream);
    assert_eq!(narrow.screen().contents().trim_end(), "ABCDEFGHIJKL");

    // vt100's set_size does NOT reflow: widening clears the wrap flags, so
    // the three physical rows become three separate logical lines instead
    // of re-joining and re-wrapping at the new width. This is the bug.
    narrow.set_size(10, 12);
    assert_eq!(
        narrow.screen().contents().trim_end(),
        "ABCD\nEFGH\nIJKL",
        "set_size unexpectedly reflowed — the bug may be fixed upstream",
    );

    // Replaying the same stream via rebuild_parser at the wide width
    // reflows correctly: the line fits on one row.
    let history: VecDeque<u8> = stream.iter().copied().collect();
    let wide = PtyManager::rebuild_parser(&history, 10, 12, 100);
    assert_eq!(wide.screen().contents().trim_end(), "ABCDEFGHIJKL");
}

#[test]
fn rebuild_parser_handles_empty_history() {
    let history: VecDeque<u8> = VecDeque::new();
    let parser = PtyManager::rebuild_parser(&history, 5, 20, 100);
    assert_eq!(parser.screen().contents().trim_end(), "");
}

/// Replaying a stream that enters and exits the alternate screen must
/// reconstruct the correct final (normal-screen) state, since alt-screen
/// transitions are pure byte sequences in the stream.
#[test]
fn rebuild_parser_reconstructs_alt_screen_roundtrip() {
    // normal text, enter alt screen, draw, exit alt screen, more normal text
    let mut stream: Vec<u8> = Vec::new();
    stream.extend_from_slice(b"normal-before\r\n");
    stream.extend_from_slice(b"\x1b[?1049h"); // enter alt screen
    stream.extend_from_slice(b"ALT-CONTENT");
    stream.extend_from_slice(b"\x1b[?1049l"); // exit alt screen
    stream.extend_from_slice(b"normal-after");

    let history: VecDeque<u8> = stream.iter().copied().collect();
    let parser = PtyManager::rebuild_parser(&history, 6, 40, 100);

    // Back on the normal screen after the roundtrip.
    assert!(!parser.screen().alternate_screen());
    let contents = parser.screen().contents();
    assert!(contents.contains("normal-before"), "got: {contents:?}");
    assert!(contents.contains("normal-after"), "got: {contents:?}");
    // Alt-screen content does not bleed into the normal grid.
    assert!(!contents.contains("ALT-CONTENT"), "got: {contents:?}");
}

#[test]
fn utf8_chunks_never_splits_a_multibyte_char() {
    // Each kana is 3 bytes in UTF-8. With max=4, a naive byte split would
    // cut the second character; utf8_chunks must keep every char intact.
    let text = "あいうえお"; // 5 chars × 3 bytes = 15 bytes
    let chunks = utf8_chunks(text, 4);
    // Reassembling the chunks must reproduce the input exactly...
    assert_eq!(chunks.concat(), text);
    // ...and every chunk must be valid (one whole 3-byte char fits in 4).
    for chunk in &chunks {
        assert_eq!(chunk.chars().count(), 1);
        assert!(chunk.len() <= 4);
    }
}

#[test]
fn utf8_chunks_preserves_mixed_and_ascii_text() {
    let text = "abc日本語def";
    // A chunk size that lands mid-character on a naive split.
    let chunks = utf8_chunks(text, 5);
    assert_eq!(chunks.concat(), text);
    for chunk in &chunks {
        assert!(chunk.len() <= 5);
        // No chunk boundary fell inside a character: re-parsing is lossless.
        assert!(std::str::from_utf8(chunk.as_bytes()).is_ok());
    }
}

#[test]
fn utf8_chunks_handles_char_wider_than_max() {
    // Defensive path: a single char larger than `max` is emitted whole
    // rather than looping forever.
    let chunks = utf8_chunks("あ", 1); // 'あ' is 3 bytes
    assert_eq!(chunks, vec!["あ"]);
}

#[test]
fn utf8_chunks_empty_input_yields_no_chunks() {
    assert!(utf8_chunks("", 1024).is_empty());
}

// ── utf8_locale_overrides ────────────────────────────────────────────

#[test]
fn locale_unset_everywhere_forces_utf8() {
    // The load-bearing case: a stripped environment makes vim default to
    // latin1, which garbles full-width input. We inject a UTF-8 LC_CTYPE.
    let (sets, removes) = utf8_locale_overrides(None, None, None);
    assert_eq!(sets, vec![("LC_CTYPE", "C.UTF-8")]);
    assert!(removes.is_empty());
}

#[test]
fn locale_empty_values_are_treated_as_unset() {
    // macOS commonly leaves LANG/LC_ALL empty; an empty value must not be
    // mistaken for an active non-UTF-8 locale.
    let (sets, removes) = utf8_locale_overrides(Some(""), Some(""), Some(""));
    assert_eq!(sets, vec![("LC_CTYPE", "C.UTF-8")]);
    assert!(removes.is_empty());
}

#[test]
fn locale_existing_utf8_is_respected() {
    // LC_CTYPE=UTF-8 (the macOS Terminal default) already yields utf-8.
    assert_eq!(
        utf8_locale_overrides(None, Some("UTF-8"), None),
        (Vec::new(), Vec::new())
    );
    // A full UTF-8 locale in LANG is honored too.
    assert_eq!(
        utf8_locale_overrides(None, None, Some("en_US.UTF-8")),
        (Vec::new(), Vec::new())
    );
    // Case-insensitive and the `utf8` spelling both count.
    assert_eq!(
        utf8_locale_overrides(None, None, Some("ja_JP.utf8")),
        (Vec::new(), Vec::new())
    );
}

#[test]
fn locale_lc_all_takes_precedence_for_detection() {
    // A UTF-8 LC_ALL wins even if LANG is a non-UTF-8 locale.
    assert_eq!(
        utf8_locale_overrides(Some("C.UTF-8"), None, Some("C")),
        (Vec::new(), Vec::new())
    );
}

#[test]
fn locale_non_utf8_lc_all_is_dropped_so_lc_ctype_can_win() {
    // LC_ALL shadows LC_CTYPE, so a non-UTF-8 LC_ALL must be removed for the
    // injected LC_CTYPE to take effect.
    let (sets, removes) = utf8_locale_overrides(Some("C"), Some("C"), Some("C"));
    assert_eq!(sets, vec![("LC_CTYPE", "C.UTF-8")]);
    assert_eq!(removes, vec!["LC_ALL"]);
}

#[test]
fn locale_non_utf8_lang_without_lc_all_keeps_lc_all_untouched() {
    let (sets, removes) = utf8_locale_overrides(None, None, Some("C"));
    assert_eq!(sets, vec![("LC_CTYPE", "C.UTF-8")]);
    assert!(removes.is_empty());
}

// ── Claude セッションフックの差し込み ──────────────────────────────────
//
// `/clear` は Claude Code の書き込み先を別 id の `.jsonl` に移すが、旧ログにも
// 新ログにも相互参照が残らない。`SessionStart` フックだけがパネルと新しい
// session id を確実に結べるので、spawn 時にこれを差し込む。

#[test]
fn hook_settings_declare_the_conductor_session_start_hook() {
    let repo = tempfile::tempdir().expect("tmp repo");
    let path = PtyManager::write_hook_settings(repo.path()).expect("write settings");

    // Conductor 自身のディレクトリに置く (`.conductor/` は gitignore 済み)。
    assert_eq!(
        path,
        repo.path().join(".conductor").join("claude-hooks.json")
    );

    let v: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).expect("read back")).expect("valid json");
    let cmd = v["hooks"]["SessionStart"][0]["hooks"][0]["command"]
        .as_str()
        .expect("hook command");
    // conductor 自身を呼ぶ。シェルスクリプトにも jq にも依存しない。
    assert!(cmd.ends_with(" cc-hook"), "{cmd}");
    assert_eq!(v["hooks"]["SessionStart"][0]["hooks"][0]["type"], "command");
}

#[test]
fn hook_settings_are_rewritten_each_spawn() {
    // conductor を置き直しても追随できるよう、毎回書き直す。
    let repo = tempfile::tempdir().expect("tmp repo");
    let path = PtyManager::write_hook_settings(repo.path()).expect("first write");
    std::fs::write(&path, b"{}").expect("clobber");
    PtyManager::write_hook_settings(repo.path()).expect("second write");

    let v: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).expect("read back")).expect("valid json");
    assert!(v["hooks"]["SessionStart"][0]["hooks"][0]["command"].is_string());
}

#[test]
fn hook_command_survives_awkward_executable_paths() {
    // フックの command はシェルに渡されるので、空白や引用符を含む置き場所でも
    // 1 語のままでなければならない。
    assert_eq!(
        PtyManager::shell_quote("/usr/bin/conductor"),
        "'/usr/bin/conductor'"
    );
    assert_eq!(
        PtyManager::shell_quote("/Users/me/my tools/conductor"),
        "'/Users/me/my tools/conductor'"
    );
    assert_eq!(
        PtyManager::shell_quote("/tmp/it's here/conductor"),
        r"'/tmp/it'\''s here/conductor'"
    );
}
