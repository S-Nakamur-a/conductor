//! ユーザの [keybinds] オーバーレイのテスト: 加算的なオーバーライド、チョードの
//! トゥームストーン、未知のアクション/レイヤー、レガシー設定形式の検出、
//! レイヤー内でのチョード衝突。

use super::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[test]
fn ユーザ設定はキーを足せる() {
    // ワークツリーレイヤーで "n" -> navigate_down をバインドする。
    let mut layer = toml::Table::new();
    layer.insert(
        "n".to_string(),
        toml::Value::String("navigate_down".to_string()),
    );
    let mut layers = toml::Table::new();
    layers.insert("worktree".to_string(), toml::Value::Table(layer));
    let mut user = toml::Table::new();
    user.insert("layers".to_string(), toml::Value::Table(layers));

    let (km, warnings) = KeyMap::with_warnings(&user);
    assert!(warnings.is_empty(), "{warnings:?}");

    // 'n' で下に移動するようになり…
    let key_n = KeyEvent::new(KeyCode::Char('n'), KeyModifiers::empty());
    assert_eq!(
        km.resolve(&key_n, KeyContext::Worktree),
        Some(Action::NavigateDown)
    );
    // …デフォルトの 'j' も引き続き動く（置き換えではなく重ね合わせ）。
    let key_j = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::empty());
    assert_eq!(
        km.resolve(&key_j, KeyContext::Worktree),
        Some(Action::NavigateDown)
    );
}

#[test]
fn ユーザ設定は既定のキーを覆う() {
    // ワークツリーで "g" -> go_to_top に再バインドする（デフォルトは grab_branch）。
    let mut layer = toml::Table::new();
    layer.insert(
        "g".to_string(),
        toml::Value::String("go_to_top".to_string()),
    );
    let mut layers = toml::Table::new();
    layers.insert("worktree".to_string(), toml::Value::Table(layer));
    let mut user = toml::Table::new();
    user.insert("layers".to_string(), toml::Value::Table(layers));

    let km = KeyMap::new(&user);
    let key_g = KeyEvent::new(KeyCode::Char('g'), KeyModifiers::empty());
    assert_eq!(
        km.resolve(&key_g, KeyContext::Worktree),
        Some(Action::GoToTop)
    );
}

#[test]
fn ユーザの打ち消しは既定のキーを外す() {
    // "ctrl+q" = false はデフォルトの Quit バインディングを完全に取り除く
    // （keymap-suite のマージにおけるトゥームストーン）。そのためこのチョードは
    // 別のアクションに覆われるのではなく、そのまま通過するようになる。これは
    // 警告面では no-op である。
    let mut keys = toml::Table::new();
    keys.insert("ctrl+q".to_string(), toml::Value::Boolean(false));
    let mut user = toml::Table::new();
    user.insert("keys".to_string(), toml::Value::Table(keys));

    let (km, warnings) = KeyMap::with_warnings(&user);
    assert!(warnings.is_empty(), "{warnings:?}");

    let ctrl_q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL);
    assert_eq!(km.resolve(&ctrl_q, KeyContext::Global), None);
    // トゥームストーンが触れていないデフォルトは引き続き解決される。
    let ctrl_n = KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL);
    assert_eq!(
        km.resolve(&ctrl_n, KeyContext::Global),
        Some(Action::NewClaudeCode)
    );
}

#[test]
fn パネル層での打ち消しも効く() {
    // トゥームストーンは名前付きレイヤーでも機能する: ワークツリーの 'c'
    // （チェリーピック）を落とす。
    let mut layer = toml::Table::new();
    layer.insert("c".to_string(), toml::Value::Boolean(false));
    let mut layers = toml::Table::new();
    layers.insert("worktree".to_string(), toml::Value::Table(layer));
    let mut user = toml::Table::new();
    user.insert("layers".to_string(), toml::Value::Table(layers));

    let (km, warnings) = KeyMap::with_warnings(&user);
    assert!(warnings.is_empty(), "{warnings:?}");
    let key_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::empty());
    assert_eq!(km.resolve(&key_c, KeyContext::Worktree), None);
}

#[test]
fn 知らないアクション名は警告になる() {
    let mut keys = toml::Table::new();
    keys.insert(
        "ctrl+z".to_string(),
        toml::Value::String("frobnicate".to_string()),
    );
    let mut user = toml::Table::new();
    user.insert("keys".to_string(), toml::Value::Table(keys));

    let (_km, warnings) = KeyMap::with_warnings(&user);
    assert!(
        warnings.iter().any(|w| matches!(
            w,
            KeybindWarning::UnknownAction { action, .. } if action == "frobnicate"
        )),
        "expected UnknownAction, got {warnings:?}"
    );
}

#[test]
fn 旧形式は黙らずに報告する() {
    // 旧スキーマ: [keybinds.worktree] navigate_down = "j" — "keys"/"layers"
    // ではなく "worktree" という名前のトップレベルテーブル。
    let mut wt = toml::Table::new();
    wt.insert(
        "navigate_down".to_string(),
        toml::Value::String("j".to_string()),
    );
    let mut user = toml::Table::new();
    user.insert("worktree".to_string(), toml::Value::Table(wt));

    let (_km, warnings) = KeyMap::with_warnings(&user);
    assert!(
        warnings
            .iter()
            .any(|w| matches!(w, KeybindWarning::InvalidConfig { .. })),
        "expected InvalidConfig, got {warnings:?}"
    );
}

#[test]
fn 同じ層の中の衝突は警告になる() {
    // 1つのレイヤー内で同じチョードの2通りの綴り: keymap-config が Conflict
    // を報告し、後の方のバインディングが勝つ。
    let mut keys = toml::Table::new();
    keys.insert(
        "ctrl+x".to_string(),
        toml::Value::String("quit".to_string()),
    );
    keys.insert(
        "control+x".to_string(),
        toml::Value::String("show_help".to_string()),
    );
    let mut user = toml::Table::new();
    user.insert("keys".to_string(), toml::Value::Table(keys));

    let (km, warnings) = KeyMap::with_warnings(&user);
    assert!(
        warnings
            .iter()
            .any(|w| matches!(w, KeybindWarning::Conflict { .. })),
        "expected Conflict, got {warnings:?}"
    );
    // どちらが勝ったにせよ、ctrl+x は2つの候補のどちらかに解決されなければならない。
    let key = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL);
    let resolved = km.resolve(&key, KeyContext::Global);
    assert!(
        matches!(resolved, Some(Action::Quit) | Some(Action::ShowHelp)),
        "got {resolved:?}"
    );
}

#[test]
fn 知らない層に割り当てがあれば警告になる() {
    // 空の GLOBAL_LAYER を抑制する仕組みを検証する: 空でない未認識のレイヤー名は
    // 警告になり、常に注入される空のレイヤーは警告にならない。
    let mut layer = toml::Table::new();
    layer.insert(
        "j".to_string(),
        toml::Value::String("navigate_down".to_string()),
    );
    let mut layers = toml::Table::new();
    layers.insert("bogus".to_string(), toml::Value::Table(layer));
    let mut user = toml::Table::new();
    user.insert("layers".to_string(), toml::Value::Table(layers));

    let (_km, warnings) = KeyMap::with_warnings(&user);
    assert!(
        warnings
            .iter()
            .any(|w| matches!(w, KeybindWarning::UnknownLayer { layer } if layer == "bogus")),
        "expected UnknownLayer, got {warnings:?}"
    );
}
