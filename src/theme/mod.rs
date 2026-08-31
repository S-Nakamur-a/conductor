//! UI カラーのテーマ設定。
//!
//! UI 全体で使う名前付きカラーの集合を定義し、設定からのカスタムテーマ読み込みにも対応する。
//! 組み込みパレットは 1 テーマ 1 ファイルで、それぞれが impl Theme のコンストラクタを持つ。

use ratatui::style::Color;

mod catppuccin;
mod color_ops;
mod dracula;
mod github;
mod gruvbox;
mod hsl;
mod kanagawa;
mod nord;
mod registry;
mod rose_pine;
mod solarized;
mod tokyo_night;

#[cfg(test)]
mod tests;

/// アプリケーションのカラーテーマ。
#[derive(Debug, Clone)]
pub struct Theme {
    // メタ情報
    /// 読むのは from_name/all_names の整合テストだけなので dead_code に見えるが、
    /// from_name("nord") が別のテーマを返すような登録ミスは、この名前と突き合わせる
    /// 以外に検出できない。全テーマのコンストラクタが代入するため cfg(test) には寄せられない。
    #[allow(dead_code)]
    pub name: &'static str,
    /// OSC 11 の背景色自動判定とテーマピッカーの明暗タグが読む。
    pub light: bool,

    // 基本色
    pub fg: Color,
    /// ハイライトや選択に使う。
    pub accent: Color,
    pub muted: Color,
    pub success: Color,
    pub error: Color,
    pub warning: Color,
    pub info: Color,

    // diff 表示
    pub diff_add: Color,
    pub diff_add_bg: Color,
    pub diff_del: Color,
    pub diff_del_bg: Color,
    /// 単語単位の強調に使う、通常の diff 背景より明るい色。
    pub diff_add_bg_emphasis: Color,
    pub diff_del_bg_emphasis: Color,
    /// ハンクのセクションヘッダ(関数名など)。
    pub diff_section_header: Color,

    // 枠線
    pub border_focused: Color,
    pub border_unfocused: Color,
    /// サブエリア間の区切り。
    pub border_secondary: Color,

    // 選択
    pub selected_bg: Color,
    pub selected_fg: Color,
    pub selected_bg_inactive: Color,
    pub selected_fg_inactive: Color,

    // 行選択 (Viewer)
    pub line_selected_bg: Color,
    pub line_selected_fg: Color,

    // 行番号ガター
    pub gutter_selected_bg: Color,
    pub gutter_selected_fg: Color,
    pub gutter_hover_fg: Color,
    pub gutter_hover_bg: Color,

    // テキスト
    pub hint: Color,
    pub search_match_fg: Color,
    pub search_match_bg: Color,
    pub search_current_fg: Color,

    // 待機中のパルス表示
    pub waiting_primary: Color,
    pub waiting_secondary: Color,

    // タイトルバー
    pub titlebar_bg: Color,
    pub dir_fg: Color,

    // ステータスバーのフラッシュ背景
    pub status_bg_success: Color,
    pub status_bg_error: Color,
    pub status_bg_warning: Color,
    pub status_bg_info: Color,

    // コメントオーバーレイ
    /// comment_preview_bg (Claude 側、中立色) と comment_user_bg は必ず違う色味に
    /// すること。署名を読まずに誰が書いたかを判別できるのはこの差だけ。
    pub comment_preview_bg: Color,
    pub comment_user_bg: Color,
    pub reply_text: Color,

    // Markdown 表示
    /// code_bg は各テーマの基調色より一段暗くすること。背後に何が描画されていても
    /// GitHub 風に凹んで見せるため。code_fg は見出し/アクセントと別系統の色にして、
    /// コード参照だと一目で分かるようにする。
    pub code_bg: Color,
    pub code_fg: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self::catppuccin_mocha()
    }
}
