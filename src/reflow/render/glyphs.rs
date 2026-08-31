//! マーカーガター用のレイアウト定数。[build](super::build)（各トランスクリプト行の先頭に
//! これらのいずれかを付ける）と [helpers](super::helpers)（それらのパディングと配置を行う）
//! で共有する。

/// 左側のマーカーガターに割り当てる表示カラム数。
pub(crate) const MARKER_COLS: usize = 2;

// これらは Claude Code 自体が使っているグリフである。⏺、✻、⎿ は unicode-width では
// 1カラムと計測されるが、多くの端末・フォントでは2カラム幅で描画される（⏺ は Emoji
// プロパティまで持つ）。これが原因で後続の文字が1カラム右にずれ、行がパネル端から
// はみ出す「scrollback のにじみ」が起きていた。ASCII への置き換えはまさにこの問題のためである。
//
// 修正の方針は Claude Code 自身が使っているものと同じで、グリフの幅を一切信用しない。
// Claude Code はグリフ直後に絶対カラム移動（CHA）を発行するので、端末側がどう解釈しようと
// 本文は本来の位置から始まる。ここでの等価な手段は、グリフの直後のセルをあえて書き込まずに
// 残すことである。ratatui の diff がこれを不連続として検出し、crossterm バックエンドが
// 絶対位置指定の MoveTo を発行する。WIDTH_AMBIGUOUS_GLYPHS、super::build::width_risk_hole、
// および super::render_tests のバイトレベルの検証を参照。

/// assistant メッセージとツール呼び出し用の箇条書き・記録マーカー
/// （Claude Code の ⏺、U+23FA）。
pub(crate) const ASSISTANT_MARKER: &str = "\u{23fa}";

/// user ターン用のプロンプトマーカー（実測: Claude Code は ❯、U+276F を表示する）。
/// ⏺/✻/⎿ と異なり、❯ は Dingbats ブロックの文字で Emoji_Presentation を持たないため
/// 幅の曖昧さがなく、直後のセルを空けておく必要もない。これは全幅の背景ブロック内に
/// 置かれるため、空セルがあると背景に切れ込みとして見えてしまう点で重要である。
pub(crate) const USER_MARKER: &str = "\u{276f}";

/// ツール結果行用の角グリフ（Claude Code の ⎿、U+23BF）。
pub(crate) const TOOL_RESULT_GLYPH: &str = "\u{23bf}";

/// thinking ブロック用のマーカー（Claude Code の ✻、U+273B）。
pub(crate) const THINKING_GLYPH: &str = "\u{273b}";

/// unicode-width では1カラムと計測されるが端末によっては2カラム幅で描画されうるグリフ
/// （⏺ は Emoji プロパティまで持つ）。これらのいずれかを含む行は、直後のセルを未書き込みの
/// ままにすることで絶対カーソル移動を強制し、実際にどれだけ幅広く描画されようと後続テキストを
/// 意図した位置に固定する（super::build::width_risk_hole を参照）。
///
/// ❯（U+276F）と ›（U+203A）は意図的にここに含めていない。どちらも Emoji presentation を
/// 持たない単なる約物であり、user ターンの ❯ は全幅の背景ブロック内にあるため、空セルは
/// 背景の切れ込みとして見えてしまう。
const WIDTH_AMBIGUOUS_GLYPHS: [char; 3] = ['\u{23fa}', '\u{23bf}', '\u{273b}'];

pub(crate) fn is_width_ambiguous(ch: char) -> bool {
    WIDTH_AMBIGUOUS_GLYPHS.contains(&ch)
}

/// 折り畳まれた <teammate-message> ブロック用のマーカー。Claude Code 自体が描画するもの
/// ではなく Conductor 独自のマルチエージェント構文である。›（U+203A、一般約物）は
/// Emoji_Presentation を持つ文字ではなく unicode_width では1カラムと計測される（この
/// クレートのバージョンで検証済み）ため、USER_MARKER と同様に空セルは不要である。
pub(crate) const TEAMMATE_MESSAGE_GLYPH: &str = "\u{203a}";
