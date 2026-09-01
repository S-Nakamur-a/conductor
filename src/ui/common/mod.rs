//! 複数のパネルで共有される、真にパネル横断の UI プリミティブ。
//!
//! [PanelChrome] は3パネル以上、[color] のバッジ/コントラスト計算と
//! [strip::visible_window] はそれぞれ2〜3箇所から使う。画面全幅のバー
//! （タイトルバー・ステータスバー・worktree ラベル）は [crate::ui::chrome]
//! にある。

pub(crate) mod color;
pub mod entrance;
mod panel_chrome;
pub mod strip;
pub mod text;

#[cfg(test)]
mod tests;

pub use panel_chrome::PanelChrome;

/// 非同期処理中に使う点字スピナーのフレーム一覧。
const BRAILLE_SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// 指定した UI tick に対応する現在のスピナーフレーム。およそ4フレームごとに進めることで
/// 安定した回転に見えるようにしている。非同期処理のスピナーを表示するすべてのパネルで
/// 共有し、アニメーションを同期させる。
pub fn spinner_frame(ui_tick: u64) -> &'static str {
    BRAILLE_SPINNER[(ui_tick as usize / 4) % BRAILLE_SPINNER.len()]
}

/// revidere の状態を表す 1 文字。幅は常に 1。
///
/// 色だけで区別すると配色や色覚によって読めなくなるので、形でも分かるように
/// してある。✓ は「作業ツリーが綺麗」「ファイルを読んだ」に既に使っていて、
/// レビューの印に流用すると git の情報と見分けが付かない。
pub fn revidere_marker(state: crate::revidere::ArtifactState, ui_tick: u64) -> &'static str {
    use crate::revidere::ArtifactState as S;
    match state {
        S::Running => spinner_frame(ui_tick),
        S::Fresh => "\u{25a4}", // ▤
        S::Stale => "!",
        S::None => "\u{25cb}", // ○
    }
}

/// revidere の状態の色。muted は複数のテーマで見えなくなるので使わない。
///
/// この色は必ず素の背景の上で使う。選択中の worktree チップのような塗りの
/// 上に重ねてはいけない — 全テーマで accent と selected_bg が同じ色なので、
/// 実行中の印が背景と完全に同色になって消える。
pub fn revidere_color(
    theme: &crate::theme::Theme,
    state: crate::revidere::ArtifactState,
) -> ratatui::style::Color {
    use crate::revidere::ArtifactState as S;
    match state {
        S::None => theme.hint,
        S::Running => theme.accent,
        S::Fresh => theme.success,
        S::Stale => theme.warning,
    }
}

/// コンテキスト内のアクションに対してユーザに見せるのに最も適したキーコード1つ:
/// 最短の ASCII のみのもの。macOS の Option グリフのフォールバック（¬, ˙, …）や
/// その他の非ASCIIキーコードもキーマップを往復はするが画面上では意味をなさないため、
/// 素のキーコードが存在する限りそちらを優先する。ステータスバーのヒント表示に加え、
/// コマンドパレットとメニューバーもキー表示にこれを使う。
pub(crate) fn representative_chord(
    keymap: &crate::keymap::KeyMap,
    context: crate::keymap::KeyContext,
    action: crate::keymap::Action,
) -> Option<String> {
    keymap
        .keys_for_action(context, action)
        .into_iter()
        .filter(|c| c.is_ascii())
        .min_by(|a, b| a.len().cmp(&b.len()).then_with(|| a.cmp(b)))
}
