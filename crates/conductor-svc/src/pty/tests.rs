//! 実 PTY を起動せずに固定できる事実だけを試す: 入力の符号化とサニタイズ、
//! 生履歴のトリムと再生、UTF-8 の分割、ロケール判定、組み立てたコマンド行。

use std::collections::VecDeque;

use super::io::{encode_mouse_wheel, sanitize_pasted_text, scroll_arrow_sequence};
use super::locale::{utf8_chunks, utf8_locale_overrides};
use super::reader::{note_shown_cursor, trim_raw_history};
use super::screen::rebuild_parser;
use super::spawn::{Conversation, Launch, build_command, relay_statusline, use_account};

#[test]
fn 隠している間のカーソル位置は取り込まない() {
    let mut parser = vt100::Parser::new(10, 40, 0);
    let mut shown = (0, 0);
    parser.process(b"\x1b[5;3H");
    note_shown_cursor(parser.screen(), &mut shown);
    assert_eq!(shown, (4, 2));

    parser.process(b"\x1b[?25l\x1b[2;1Hstreaming");
    note_shown_cursor(parser.screen(), &mut shown);
    assert_eq!(shown, (4, 2), "描き途中の位置は無視する");

    parser.process(b"\x1b[8;3H\x1b[?25h");
    note_shown_cursor(parser.screen(), &mut shown);
    assert_eq!(shown, (7, 2));
}

#[test]
fn 矢印の列はdecckmと向きに従う() {
    // アプリケーションカーソルキーモードは SS3 (ESC O)、通常は CSI (ESC [)。
    let cases = [
        (true, true, &b"\x1bOA"[..]),
        (false, true, &b"\x1bOB"[..]),
        (true, false, &b"\x1b[A"[..]),
        (false, false, &b"\x1b[B"[..]),
    ];
    for (up, app_cursor, want) in cases {
        assert_eq!(
            scroll_arrow_sequence(up, app_cursor),
            want,
            "{up} {app_cursor}"
        );
    }
}

#[test]
fn 貼り付けは本文だけ残す() {
    let cases = [
        // 素のテキスト、タブ、改行、マルチバイトはそのまま。
        (
            "echo hello\tworld\nsecond line\n",
            "echo hello\tworld\nsecond line\n",
        ),
        ("こんにちは\tworld\n", "こんにちは\tworld\n"),
        // CR と CRLF は LF に正規化する。
        ("a\r\nb\rc", "a\nb\nc"),
        // CSI (SGR カラー) は丸ごと落ちる。
        ("\x1b[31mred\x1b[0m text", "red text"),
        // BEL 終端の OSC (ウィンドウタイトル) も丸ごと落ちる。
        ("before\x1b]0;evil title\x07after", "beforeafter"),
        // 裸の制御バイト (NUL / BEL / BS / DEL) は消え、本文は無傷。
        ("a\x00b\x07c\x08d\x7fe", "abcde"),
    ];
    for (input, want) in cases {
        assert_eq!(sanitize_pasted_text(input), want, "{input:?}");
    }
}

/// 挙動を左右する重要ケース: paste-end マーカーの後にコマンドを忍ばせた
/// クリップボードが bracketed paste から抜け出せてはならない。
#[test]
fn 紛れ込んだ貼り付け終了マーカーを取り除く() {
    let out = sanitize_pasted_text("safe\x1b[201~rm -rf /\n");
    assert_eq!(out, "saferm -rf /\n");
    assert!(!out.contains("201~"));
    assert!(!out.contains('\x1b'));
}

#[test]
fn ホイールの符号化は方式と向きに従う() {
    use vt100::MouseProtocolEncoding::{Default, Sgr};
    let cases = [
        // SGR: アップ = ボタン 64、ダウン = 65。
        (true, 5, 9, Sgr, b"\x1b[<64;5;9M".to_vec()),
        (false, 1, 1, Sgr, b"\x1b[<65;1;1M".to_vec()),
        // 座標 0 は 1 にクランプして well-formed に保つ。
        (true, 0, 0, Sgr, b"\x1b[<64;1;1M".to_vec()),
        // レガシー X10 は各値を 32 ずらす (64+32 = 96)。
        (true, 2, 3, Default, vec![0x1b, b'[', b'M', 96, 34, 35]),
    ];
    for (up, col, row, encoding, want) in cases {
        assert_eq!(
            encode_mouse_wheel(up, col, row, encoding),
            want,
            "{up} {col} {row}"
        );
    }
}

#[test]
fn 履歴は上限以下かつ行境界に揃える() {
    let mut history: VecDeque<u8> = b"aaaa\nbbbb\ncccc\ndddd\n".iter().copied().collect();
    trim_raw_history(&mut history, 10);

    let text = String::from_utf8(history.into_iter().collect()).unwrap();
    assert!(text.len() <= 10, "{text:?}");
    assert!(!text.contains("aaaa"), "{text:?}");
    // 残った完全な行はどれも元のまま (先頭に欠けた行が残っていない)。
    for line in text.split_inclusive('\n') {
        if line.ends_with('\n') {
            assert!(["bbbb\n", "cccc\n", "dddd\n"].contains(&line), "{line:?}");
        }
    }
}

#[test]
fn 上限以下の履歴には触らない() {
    for (bytes, cap) in [(&b"hello\n"[..], 1024), (&b"abcd\n"[..], 5), (&b""[..], 0)] {
        let mut history: VecDeque<u8> = bytes.iter().copied().collect();
        trim_raw_history(&mut history, cap);
        assert_eq!(history.into_iter().collect::<Vec<u8>>(), bytes, "cap={cap}");
    }
}

/// 改行を一切含まない超過バッファを空まで削ってはならない。リサイズで画面が真っ白に
/// なる方が、先頭行が欠けるよりはるかに悪い。エスケープだらけの TUI 出力で起きる。
#[test]
fn 改行が無くても中身は空にしない() {
    let mut history: VecDeque<u8> = std::iter::repeat_n(b'x', 100).collect();
    trim_raw_history(&mut history, 10);
    assert!(!history.is_empty(), "history was drained to empty");
    assert!(history.len() <= 10);
    assert!(history.iter().all(|&b| b == b'x'));
}

/// 再生が新しい幅で折り返し直すことが、リサイズ経路の核心。vt100 自身の set_size は
/// リフローしない — その事実もここで固定しておく。
#[test]
fn 再生は新しい幅で折り返し直す() {
    // 明示的な改行の無い 12 文字の論理行。
    let stream = b"ABCDEFGHIJKL";

    let mut narrow = vt100::Parser::new(10, 4, 100);
    narrow.process(stream);
    assert_eq!(narrow.screen().contents().trim_end(), "ABCDEFGHIJKL");

    narrow.set_size(10, 12);
    assert_eq!(
        narrow.screen().contents().trim_end(),
        "ABCD\nEFGH\nIJKL",
        "set_size unexpectedly reflowed — the bug may be fixed upstream",
    );

    let history: VecDeque<u8> = stream.iter().copied().collect();
    let wide = rebuild_parser(&history, 10, 12, 100);
    assert_eq!(wide.screen().contents().trim_end(), "ABCDEFGHIJKL");
}

#[test]
fn 空の履歴でもパーサを組み直せる() {
    let parser = rebuild_parser(&VecDeque::new(), 5, 20, 100);
    assert_eq!(parser.screen().contents().trim_end(), "");
}

/// オルタネート画面の出入りはストリーム中の純粋なバイト列なので、再生すれば
/// 正しい最終状態 (通常画面) が組み上がる。
#[test]
fn 代替画面の往復も組み直せる() {
    let mut stream: Vec<u8> = Vec::new();
    stream.extend_from_slice(b"normal-before\r\n");
    stream.extend_from_slice(b"\x1b[?1049h");
    stream.extend_from_slice(b"ALT-CONTENT");
    stream.extend_from_slice(b"\x1b[?1049l");
    stream.extend_from_slice(b"normal-after");

    let parser = rebuild_parser(&stream.into_iter().collect(), 6, 40, 100);
    assert!(!parser.screen().alternate_screen());
    let contents = parser.screen().contents();
    assert!(contents.contains("normal-before"), "got: {contents:?}");
    assert!(contents.contains("normal-after"), "got: {contents:?}");
    // オルタネート画面の中身は通常のグリッドに漏れない。
    assert!(!contents.contains("ALT-CONTENT"), "got: {contents:?}");
}

#[test]
fn 分割はマルチバイト文字を割らない() {
    // 単純なバイト分割なら文字の途中に着地する組み合わせを並べる。
    for (text, max) in [
        ("あいうえお", 4),
        ("abc日本語def", 5),
        ("あ", 1),
        ("", 1024),
    ] {
        let chunks = utf8_chunks(text, max);
        assert_eq!(chunks.concat(), text, "{text:?} max={max}");
        for chunk in &chunks {
            assert!(!chunk.is_empty(), "{text:?} max={max}");
            // max を超えられるのは、1 文字がそれより大きいときだけ。
            assert!(
                chunk.len() <= max || chunk.chars().count() == 1,
                "{chunk:?} max={max}"
            );
        }
    }
}

#[test]
fn ロケール判定はposixの優先順位に従う() {
    const UTF8: (&str, &str) = ("LC_CTYPE", "C.UTF-8");
    type Env = Option<&'static str>;
    type Overrides = (Vec<(&'static str, &'static str)>, Vec<&'static str>);
    // (LC_ALL, LC_CTYPE, LANG) → (設定する, 削除する)
    let cases: [(Env, Env, Env, Overrides); 7] = [
        // 何も無ければ vim が latin1 に落ちるので UTF-8 を注入する。
        (None, None, None, (vec![UTF8], vec![])),
        // macOS では空のままなことが多い。空は未設定と同じで、非 UTF-8 ではない。
        (Some(""), Some(""), Some(""), (vec![UTF8], vec![])),
        // すでに UTF-8 ならユーザの設定を尊重する (綴りと大小文字は問わない)。
        (None, Some("UTF-8"), None, (vec![], vec![])),
        (None, None, Some("en_US.UTF-8"), (vec![], vec![])),
        (None, None, Some("ja_JP.utf8"), (vec![], vec![])),
        // LC_ALL が LANG より優先される。
        (Some("C.UTF-8"), None, Some("C"), (vec![], vec![])),
        // 非 UTF-8 の LC_ALL は注入した LC_CTYPE を覆い隠すので消す。
        (
            Some("C"),
            Some("C"),
            Some("C"),
            (vec![UTF8], vec!["LC_ALL"]),
        ),
    ];
    for (lc_all, lc_ctype, lang, want) in cases {
        assert_eq!(
            utf8_locale_overrides(lc_all, lc_ctype, lang),
            want,
            "{lc_all:?} {lc_ctype:?} {lang:?}"
        );
    }
}

/// LC_ALL が無ければ、LANG が非 UTF-8 でも LC_ALL に触らない。
#[test]
fn 無いlc_allは削除対象にしない() {
    let (sets, removes) = utf8_locale_overrides(None, None, Some("C"));
    assert_eq!(sets, vec![("LC_CTYPE", "C.UTF-8")]);
    assert!(removes.is_empty());
}

#[test]
fn フックを降りたセッションはsettingsを渡さず書きもしない() {
    let repo = tempfile::tempdir().unwrap();
    let settings = repo.path().join(".conductor/claude-hooks.json");
    let claude = |session_hooks| Launch::ClaudeCode {
        repo_root: repo.path(),
        conversation: Conversation::New,
        session_name: None,
        session_hooks,
        chrome: false,
        account: None,
    };
    let passes_settings = |launch: &Launch<'_>| {
        let (cmd, _) = build_command(launch, "panel");
        cmd.get_argv().iter().any(|arg| arg == "--settings")
    };

    assert!(passes_settings(&claude(true)));
    assert!(settings.exists());

    std::fs::remove_file(&settings).unwrap();
    assert!(!passes_settings(&claude(false)));
    assert!(
        !settings.exists(),
        "先客のパネルが次に読む設定を書き換えてはいけない"
    );
}

#[test]
fn chromeを頼んだセッションだけがchromeフラグを渡す() {
    let repo = tempfile::tempdir().unwrap();
    let passes_chrome = |chrome| {
        let launch = Launch::ClaudeCode {
            repo_root: repo.path(),
            conversation: Conversation::New,
            session_name: None,
            session_hooks: false,
            chrome,
            account: None,
        };
        let (cmd, _) = build_command(&launch, "panel");
        cmd.get_argv().iter().any(|arg| arg == "--chrome")
    };

    assert!(passes_chrome(true));
    assert!(!passes_chrome(false));
}

#[test]
fn 最初のプロンプトは新しい会話の最後の位置引数で渡し_再開では渡さない() {
    let repo = tempfile::tempdir().unwrap();
    let argv = |conversation| {
        let launch = Launch::ClaudeCode {
            repo_root: repo.path(),
            conversation,
            session_name: Some("name"),
            session_hooks: true,
            chrome: false,
            account: None,
        };
        let (cmd, _) = build_command(&launch, "panel");
        cmd.get_argv()
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
    };

    let with = argv(Conversation::NewWith("/conductor:revidere"));
    assert_eq!(with.last().unwrap(), "/conductor:revidere");
    assert!(with.iter().any(|a| a == "--session-id"));
    assert!(with.iter().any(|a| a == "--settings"));

    let plain = argv(Conversation::New);
    assert_eq!(plain.len(), with.len() - 1);
    assert!(!plain.iter().any(|a| a == "/conductor:revidere"));

    let resumed = argv(Conversation::Resume("abc"));
    assert!(resumed.windows(2).any(|w| w == ["--resume", "abc"]));
    assert!(!resumed.iter().any(|a| a == "--session-id"));
}

#[test]
fn フックを降りてもpanel_idは渡しソケットは渡さない() {
    const SENTINEL: &str = "CONDUCTOR_TEST_SOCK_SENTINEL";
    let Ok(sentinel) = std::env::var(SENTINEL) else {
        // 継いだ宛先を消す動きは、宛先が環境にある時しか観測できない。set_var は同じ
        // プロセスの他のテストに漏れるので、環境を足した子として入り直す。
        let dir = tempfile::tempdir().unwrap();
        let sentinel = dir.path().join("entered");
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "pty::tests::フックを降りてもpanel_idは渡しソケットは渡さない",
            ])
            .env(SENTINEL, &sentinel)
            .env(
                conductor_core::cc_hook::NOTIFY_SOCK_ENV,
                "/tmp/first-window.sock",
            )
            .status()
            .unwrap();
        assert!(status.success());
        // libtest はフィルタが 1 件も当たらなくても 0 で終わるので、成功だけでは足りない。
        assert!(
            sentinel.exists(),
            "子が本体に入っていない — 上の綴りがテスト名とずれている"
        );
        return;
    };

    let repo = tempfile::tempdir().unwrap();
    let (cmd, _) = build_command(
        &Launch::ClaudeCode {
            repo_root: repo.path(),
            conversation: Conversation::New,
            session_name: None,
            session_hooks: false,
            chrome: false,
            account: None,
        },
        "panel",
    );
    // extra は自分で足した分、full は子が実際に見る分。
    let set: Vec<&str> = cmd.iter_extra_env_as_str().map(|(key, _)| key).collect();
    let seen: Vec<&str> = cmd.iter_full_env_as_str().map(|(key, _)| key).collect();

    assert!(
        set.contains(&conductor_core::cc_hook::PANEL_ID_ENV),
        "プラグインのフックはこれが無いと先客のソケットへ自力で送る"
    );
    assert!(
        !seen.contains(&conductor_core::cc_hook::NOTIFY_SOCK_ENV),
        "継いだ宛先を消さないと 2 つ目のウィンドウのパネルが先客のソケットを持つ"
    );
    std::fs::write(sentinel, "").unwrap();
}

#[test]
fn claude_には写しの置き場を渡し_窓から継いだ中継の印は消す() {
    use conductor_core::cc_hook::statusline;
    let mut cmd = portable_pty::CommandBuilder::new("claude");
    cmd.env(statusline::RELAYING_ENV, "1");
    relay_statusline(&mut cmd);
    assert_eq!(cmd.get_env(statusline::RELAYING_ENV), None);
    assert_eq!(
        cmd.get_env(statusline::DIR_ENV),
        Some(statusline::dir().as_os_str())
    );
}

#[test]
fn アカウントの_config_dir_は綴りのまま渡し_既定のアカウントは継いだ値を外し_accounts_が無ければ触らない()
 {
    use conductor_core::cc_hook::statusline::CONFIG_DIR_ENV;
    use conductor_core::config::ClaudeAccount;
    let started_from = |account: Option<&ClaudeAccount>| {
        let mut cmd = portable_pty::CommandBuilder::new("claude");
        cmd.env(CONFIG_DIR_ENV, "/inherited");
        use_account(&mut cmd, account);
        cmd.get_env(CONFIG_DIR_ENV).map(|v| v.to_owned())
    };
    let personal = ClaudeAccount {
        config_dir: Some("/home/me/.claude-personal/".into()),
        ..Default::default()
    };

    assert_eq!(
        started_from(Some(&personal)),
        Some("/home/me/.claude-personal/".into())
    );
    assert_eq!(started_from(Some(&ClaudeAccount::default())), None);
    assert_eq!(started_from(None), Some("/inherited".into()));
}
