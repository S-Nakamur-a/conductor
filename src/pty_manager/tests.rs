//! pty_manager の単体テスト: スクロール矢印のエンコード、ペーストのサニタイズ、
//! マウスホイールのエンコード、生履歴のトリム/リフロー、UTF-8 チャンク分割、
//! ロケール上書き検出。

use std::collections::VecDeque;

use super::io::{encode_mouse_wheel, sanitize_pasted_text, scroll_arrow_sequence};
use super::locale::{utf8_chunks, utf8_locale_overrides};
use super::PtyManager;

#[test]
fn arrow_sequence_honors_decckm_and_direction() {
    // アプリケーションカーソルキーモード → SS3 (ESC O)。文字 O (0x4f) に注意。
    assert_eq!(scroll_arrow_sequence(true, true), b"\x1bOA");
    assert_eq!(scroll_arrow_sequence(false, true), b"\x1bOB");
    // 通常モード → CSI (ESC [)。
    assert_eq!(scroll_arrow_sequence(true, false), b"\x1b[A");
    assert_eq!(scroll_arrow_sequence(false, false), b"\x1b[B");
}

// sanitize_pasted_text

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
    // 単語を囲む SGR カラーコードは除去され、プレーンテキストだけが残る。
    let input = "\x1b[31mred\x1b[0m text";
    assert_eq!(sanitize_pasted_text(input), "red text");
}

#[test]
fn sanitize_removes_embedded_bracketed_paste_end_marker() {
    // 挙動を左右する重要なセキュリティケース: paste-end マーカー
    // (\x1b[201~) の後にコマンドを忍ばせたクリップボードが、bracketed
    // paste から抜け出せてはならない。CSI シーケンス全体(終端の ~ を
    // 含む)を丸ごと落とすので、201~ は一切残らない。
    let input = "safe\x1b[201~rm -rf /\n";
    let out = sanitize_pasted_text(input);
    assert_eq!(out, "saferm -rf /\n");
    assert!(!out.contains("201~"));
    assert!(!out.contains('\x1b'));
}

#[test]
fn sanitize_strips_osc_string_sequence() {
    // BEL で終端される OSC(ウィンドウタイトル)— 丸ごと除去される。
    let input = "before\x1b]0;evil title\x07after";
    assert_eq!(sanitize_pasted_text(input), "beforeafter");
}

#[test]
fn sanitize_drops_bare_control_bytes_but_keeps_text() {
    // NUL、BEL、バックスペース、DEL はすべて除去され、前後のテキストは無傷。
    let input = "a\x00b\x07c\x08d\x7fe";
    assert_eq!(sanitize_pasted_text(input), "abcde");
}

#[test]
fn sanitize_preserves_multibyte_text() {
    let input = "こんにちは\tworld\n";
    assert_eq!(sanitize_pasted_text(input), input);
}

// encode_mouse_wheel

#[test]
fn encode_mouse_wheel_sgr_up_and_down() {
    // SGR: ホイールアップ = ボタン 64、ダウン = 65。座標はインラインで最後に 'M'。
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
    // レガシー X10: CSI M Cb Cx Cy で各値に +32。アップ = 64+32 = 96 = ''。
    let seq = encode_mouse_wheel(true, 2, 3, vt100::MouseProtocolEncoding::Default);
    assert_eq!(seq, vec![0x1b, b'[', b'M', 96, 34, 35]);
}

#[test]
fn encode_mouse_wheel_clamps_coordinates_to_at_least_one() {
    // 座標 0 は 1 にクランプされ、SGR 形式が well-formed であり続ける。
    assert_eq!(
        encode_mouse_wheel(true, 0, 0, vt100::MouseProtocolEncoding::Sgr),
        b"\x1b[<64;1;1M".to_vec()
    );
}

#[test]
fn trim_raw_history_keeps_under_cap_and_aligns_to_line() {
    let mut history: VecDeque<u8> = b"aaaa\nbbbb\ncccc\ndddd\n".iter().copied().collect();
    // 現在の長さを下回る上限を指定してトリムを強制する。
    PtyManager::trim_raw_history(&mut history, 10);
    let remaining: Vec<u8> = history.iter().copied().collect();
    // 上限以下でなければならない…
    assert!(remaining.len() <= 10, "len={}", remaining.len());
    // …かつ、きれいな行境界から再開している(先頭に不完全な行が無い)。
    let text = String::from_utf8(remaining).unwrap();
    for line in text.split_inclusive('\n') {
        if line.ends_with('\n') {
            // 保持されている完全な行はすべて元のいずれかである。
            assert!(["aaaa\n", "bbbb\n", "cccc\n", "dddd\n"].contains(&line));
        }
    }
    // 最も古い内容("aaaa")は削られていなければならない。
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

/// 改行を一切含まない上限超過バッファは、空になるまで削られては*ならない* —
/// リサイズ時に画面が真っ白になる方が、多少不完全な先頭行よりはるかに悪い。
/// (これはエスケープシーケンスだらけの TUI 出力での失敗モードである。)
#[test]
fn trim_raw_history_keeps_bytes_when_no_newline() {
    let mut history: VecDeque<u8> = std::iter::repeat_n(b'x', 100).collect();
    PtyManager::trim_raw_history(&mut history, 10);
    assert!(!history.is_empty(), "history was drained to empty");
    assert!(history.len() <= 10);
    assert!(history.iter().all(|&b| b == b'x'));
}

/// 生バイトストリームを新しい幅の新規パーサへ再生すると、元は旧幅で
/// ラップされていた内容がリフローされなければならない — これが
/// resize_session の列変更経路の核心である。(vt100 自身の set_size は
/// リフローしない。これがこの変更全体で修正しているバグである。)
#[test]
fn replay_reflows_to_new_width() {
    // 明示的な改行の無い、単一の12文字の論理行。
    let stream = b"ABCDEFGHIJKL";

    // 狭いパーサ(cols=4): 行は3つの物理行にまたがってラップされるが、
    // vt100 はラップを追跡しているので依然として1つの論理行である。
    let mut narrow = vt100::Parser::new(10, 4, 100);
    narrow.process(stream);
    assert_eq!(narrow.screen().contents().trim_end(), "ABCDEFGHIJKL");

    // vt100 の set_size はリフロー*しない*: 幅を広げるとラップフラグが
    // クリアされるため、3つの物理行は新しい幅で結合・再ラップされる
    // のではなく、3つの独立した論理行になってしまう。これがそのバグ。
    narrow.set_size(10, 12);
    assert_eq!(
        narrow.screen().contents().trim_end(),
        "ABCD\nEFGH\nIJKL",
        "set_size unexpectedly reflowed — the bug may be fixed upstream",
    );

    // 同じストリームを rebuild_parser 経由で広い幅に再生すると正しく
    // リフローする: 行が1つの行に収まる。
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

/// オルタネート画面への出入りを含むストリームを再生すると、遷移が
/// ストリーム中の純粋なバイトシーケンスであるため、正しい最終状態
/// (通常画面)が再構築されなければならない。
#[test]
fn rebuild_parser_reconstructs_alt_screen_roundtrip() {
    // 通常テキスト、オルタネート画面へ突入、描画、オルタネート画面を退出、通常テキスト続き
    let mut stream: Vec<u8> = Vec::new();
    stream.extend_from_slice(b"normal-before\r\n");
    stream.extend_from_slice(b"\x1b[?1049h"); // オルタネート画面へ突入
    stream.extend_from_slice(b"ALT-CONTENT");
    stream.extend_from_slice(b"\x1b[?1049l"); // オルタネート画面を退出
    stream.extend_from_slice(b"normal-after");

    let history: VecDeque<u8> = stream.iter().copied().collect();
    let parser = PtyManager::rebuild_parser(&history, 6, 40, 100);

    // 往復後は通常画面に戻っている。
    assert!(!parser.screen().alternate_screen());
    let contents = parser.screen().contents();
    assert!(contents.contains("normal-before"), "got: {contents:?}");
    assert!(contents.contains("normal-after"), "got: {contents:?}");
    // オルタネート画面の内容は通常のグリッドに漏れ出さない。
    assert!(!contents.contains("ALT-CONTENT"), "got: {contents:?}");
}

#[test]
fn utf8_chunks_never_splits_a_multibyte_char() {
    // 各かなは UTF-8 で3バイト。max=4 では、単純なバイト分割は2文字目を
    // 切ってしまう。utf8_chunks はすべての文字を無傷に保たなければならない。
    let text = "あいうえお"; // 5文字 × 3バイト = 15バイト
    let chunks = utf8_chunks(text, 4);
    // チャンクを結合すると入力と完全に一致しなければならない…
    assert_eq!(chunks.concat(), text);
    // …そしてすべてのチャンクは有効でなければならない(3バイト文字1つが4に収まる)。
    for chunk in &chunks {
        assert_eq!(chunk.chars().count(), 1);
        assert!(chunk.len() <= 4);
    }
}

#[test]
fn utf8_chunks_preserves_mixed_and_ascii_text() {
    let text = "abc日本語def";
    // 単純な分割だと文字の途中に着地するチャンクサイズ。
    let chunks = utf8_chunks(text, 5);
    assert_eq!(chunks.concat(), text);
    for chunk in &chunks {
        assert!(chunk.len() <= 5);
        // チャンクの境界が文字の内部に落ちていない: 再パースが無損失である。
        assert!(std::str::from_utf8(chunk.as_bytes()).is_ok());
    }
}

#[test]
fn utf8_chunks_handles_char_wider_than_max() {
    // 防御的な経路: max より大きい単一の文字は、無限ループせずに
    // そのまま丸ごと出力される。
    let chunks = utf8_chunks("あ", 1); // 'あ' は3バイト
    assert_eq!(chunks, vec!["あ"]);
}

#[test]
fn utf8_chunks_empty_input_yields_no_chunks() {
    assert!(utf8_chunks("", 1024).is_empty());
}

// utf8_locale_overrides

#[test]
fn locale_unset_everywhere_forces_utf8() {
    // 挙動を左右する重要なケース: 何も設定されていない環境では vim が
    // latin1 をデフォルトにし、全角入力が化ける。UTF-8 の LC_CTYPE を注入する。
    let (sets, removes) = utf8_locale_overrides(None, None, None);
    assert_eq!(sets, vec![("LC_CTYPE", "C.UTF-8")]);
    assert!(removes.is_empty());
}

#[test]
fn locale_empty_values_are_treated_as_unset() {
    // macOS では LANG/LC_ALL が空のままなことがよくある。空の値を有効な
    // 非 UTF-8 ロケールと誤認してはならない。
    let (sets, removes) = utf8_locale_overrides(Some(""), Some(""), Some(""));
    assert_eq!(sets, vec![("LC_CTYPE", "C.UTF-8")]);
    assert!(removes.is_empty());
}

#[test]
fn locale_existing_utf8_is_respected() {
    // LC_CTYPE=UTF-8 (macOS Terminal のデフォルト) はすでに utf-8 になっている。
    assert_eq!(
        utf8_locale_overrides(None, Some("UTF-8"), None),
        (Vec::new(), Vec::new())
    );
    // LANG の完全な UTF-8 ロケールも尊重される。
    assert_eq!(
        utf8_locale_overrides(None, None, Some("en_US.UTF-8")),
        (Vec::new(), Vec::new())
    );
    // 大文字小文字を区別せず、utf8 という綴りも認識する。
    assert_eq!(
        utf8_locale_overrides(None, None, Some("ja_JP.utf8")),
        (Vec::new(), Vec::new())
    );
}

#[test]
fn locale_lc_all_takes_precedence_for_detection() {
    // LANG が非 UTF-8 ロケールでも、UTF-8 の LC_ALL が優先される。
    assert_eq!(
        utf8_locale_overrides(Some("C.UTF-8"), None, Some("C")),
        (Vec::new(), Vec::new())
    );
}

#[test]
fn locale_non_utf8_lc_all_is_dropped_so_lc_ctype_can_win() {
    // LC_ALL は LC_CTYPE を覆い隠すため、注入した LC_CTYPE を効かせるには
    // 非 UTF-8 の LC_ALL を削除しなければならない。
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

// Claude セッションフックの差し込み
//
// /clear は Claude Code の書き込み先を別 id の .jsonl に移すが、旧ログにも
// 新ログにも相互参照が残らない。SessionStart フックだけがパネルと新しい
// session id を確実に結べるので、spawn 時にこれを差し込む。

#[test]
fn hook_settings_declare_the_conductor_session_start_hook() {
    let repo = tempfile::tempdir().expect("tmp repo");
    let path = PtyManager::write_hook_settings(repo.path()).expect("write settings");

    // Conductor 自身のディレクトリに置く (.conductor/ は gitignore 済み)。
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
