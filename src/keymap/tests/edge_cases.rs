//! チョード正規化のエッジケース（SHIFT の畳み込み、macOS Option の複数バイト
//! グリフ、正規形式の大文字小文字）と、特定のコンテキストやコンフィグの
//! オーバーライド挙動に紐付かないその他の細かなガードのテスト。

use super::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use keymap_suite::ActionName;

#[test]
fn keys_for_actionは正規の綴りを並べる() {
    let km = default_keymap();
    let keys = km.keys_for_action(KeyContext::Worktree, Action::NavigateDown);
    assert!(keys.contains(&"j".to_string()), "{keys:?}");
    assert!(keys.contains(&"down".to_string()), "{keys:?}");
}

#[test]
fn アクション名は全バリアントで往復する() {
    // マクロが生成する ActionName の実装は ALL 全体で全単射でなければならない —
    // 手で選んだサンプルではなく全バリアントを網羅する。名前とマッチアームが
    // 同じ宣言から生成されるようになったため。
    for &action in Action::ALL {
        assert_eq!(Action::from_name(action.name()), Some(action));
    }
}

#[test]
fn shift付きの小文字は大文字に直されない() {
    // 旧来の手書きノーマライザからの挙動の差分で、これは意図した固定挙動:
    // keymap-core はグリフをそのまま信頼し、冗長な単独 SHIFT だけを落とす。
    // そのため 'g'+SHIFT は Char('g') のままで、素の 'g' バインディング
    // （GoToTop）に当たる — 'G'（GoToBottom）に再キャスされることは
    // ない。実際の解決済みグリフ 'G' を送ってくる端末（一般的なケース）は
    // それでも GoToBottom に当たる。shift_g_resolves_uppercase_binding を参照。
    let km = default_keymap();
    let key = KeyEvent::new(KeyCode::Char('g'), KeyModifiers::SHIFT);
    assert_eq!(
        km.resolve(&key, KeyContext::Worktree),
        Some(Action::GoToTop)
    );
}

#[test]
fn macosのunicodeフォールバックのキーが解決する() {
    // これらのグリフは TOML 上では見た目では検出できない。file→keymap-config→
    // keymap-core→crossterm という経路が、plain-Option と Shift-Option の
    // どちらの系統でも複数バイト文字を通せることをこのテストで確認する。
    let km = default_keymap();
    let cases = [
        ('˙', Action::CycleFocusBackward),
        ('¬', Action::CycleFocusForward),
        ('¡', Action::FocusWorktree),
        ('§', Action::FocusTerminalShell),
        ('÷', Action::TogglePanelOverlay),
    ];
    for (glyph, action) in cases {
        let key = KeyEvent::new(KeyCode::Char(glyph), KeyModifiers::empty());
        assert_eq!(
            km.resolve(&key, KeyContext::Global),
            Some(action),
            "glyph {glyph:?}"
        );
    }
}

#[test]
fn alt_shift付きの数字はalt付き数字に潰れない() {
    // 「他の修飾キーが押されている間は SHIFT を保持する」というルール:
    // alt+1 はワークツリーにフォーカスするが、alt+shift+1 は SHIFT を落として
    // それに畳み込まれてはならない。alt+shift+digit は今は未バインド
    // （フォーカス+拡大は撤去済み）なので、正しいリゾルバは FocusWorktree に
    // 畳み込まず None を返す。
    let km = default_keymap();
    let alt_1 = KeyEvent::new(KeyCode::Char('1'), KeyModifiers::ALT);
    let alt_shift_1 = KeyEvent::new(KeyCode::Char('1'), KeyModifiers::ALT | KeyModifiers::SHIFT);
    assert_eq!(
        km.resolve(&alt_1, KeyContext::Global),
        Some(Action::FocusWorktree)
    );
    assert_eq!(km.resolve(&alt_shift_1, KeyContext::Global), None);
}

#[test]
fn diffモードではenterとshift_enterが別物() {
    // 1つのレイヤー内での、名前付きキーに対する SHIFT の判別。
    let km = default_keymap();
    let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::empty());
    let shift_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT);
    assert_eq!(
        km.resolve(&enter, KeyContext::ViewerDiffMode),
        Some(Action::ExpandContext)
    );
    assert_eq!(
        km.resolve(&shift_enter, KeyContext::ViewerDiffMode),
        Some(Action::ExpandAllContext)
    );
}

#[test]
fn keys_for_actionは小文字の正規形を使う() {
    // ヘルプ画面はこれをそのままレンダリングする。旧来の "Ctrl+d" から
    // keymap-core の正規形 "ctrl+d" に変わった大文字小文字を固定する。
    let km = default_keymap();
    let keys = km.keys_for_action(KeyContext::Viewer, Action::ScrollHalfPageDown);
    assert_eq!(keys, vec!["ctrl+d".to_string()]);
}

#[test]
fn 割り当ての無いキーは素通しする() {
    // 中立な表現を持たないキー（CapsLock）は KeyInput::try_from に失敗し、
    // パニックせず None（「そのまま通す」）に解決されなければならない。
    let km = default_keymap();
    let key = KeyEvent::new(KeyCode::CapsLock, KeyModifiers::empty());
    assert_eq!(km.resolve(&key, KeyContext::Terminal), None);
}

#[test]
fn 削除したlspのアクションはもう解釈されない() {
    // 配線されていないアクションは語彙から取り除かれたので、それをバインド
    // しようとすると黙って何もしないのではなく警告（UnknownAction）になる。
    assert_eq!(Action::from_name("go_to_definition"), None);
    assert_eq!(Action::from_name("go_to_implementation"), None);
    assert_eq!(Action::from_name("find_references"), None);
}

#[test]
fn fキーは割り当てが外れている() {
    let km = default_keymap();
    for n in 2..=7 {
        let key = KeyEvent::new(KeyCode::F(n), KeyModifiers::empty());
        assert_eq!(km.resolve(&key, KeyContext::Global), None, "F{n}");
    }
}
