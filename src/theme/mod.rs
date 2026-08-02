//! UI カラーのテーマ設定。
//!
//! UI 全体で使う名前付きカラーの集合を定義し、設定からのカスタムテーマ読み込みにも対応する。
//!
//! 責務ごとにファイルを分割している: このファイルは Theme 構造体本体と Default 実装を
//! 持つ。registry はテーマ名を組み込みパレットに解決する。color_ops は汎用のカラー演算
//! メソッド(darken/lighten/complement/lerp/high_contrast)を持つ。hsl は complement
//! が使う非公開の RGB↔HSL 変換を持つ。組み込みパレットごとのファイル
//! (catppuccin, dracula, nord, solarized, tokyo_night, gruvbox,
//! rose_pine, kanagawa, github) はそれぞれのテーマのコンストラクタを impl Theme
//! ブロックとして持つ。

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
    /// このテーマの正式名(from_name のキーおよび all_names の返り値と一致する)。
    /// 主に from_name と all_names の登録漏れをテストで検出するために使う。
    #[allow(dead_code)]
    pub name: &'static str,
    /// 明るい背景のテーマなら true、暗い背景なら false。
    /// OSC 11 自動判定とテーマピッカーの明暗タグで使う。
    pub light: bool,

    // 基本色
    /// 通常テキストの前景色。
    pub fg: Color,
    /// アクセントカラー(ハイライトや選択に使う)。
    pub accent: Color,
    /// 目立たない/薄いテキストの色。
    pub muted: Color,
    /// 成功表示の色。
    pub success: Color,
    /// エラー/危険表示の色。
    pub error: Color,
    /// 警告表示の色。
    pub warning: Color,
    /// 情報テキストの色。
    pub info: Color,

    // diff 表示
    /// diff の追加行の色。
    pub diff_add: Color,
    /// 追加行の背景色。
    pub diff_add_bg: Color,
    /// diff の削除行の色。
    pub diff_del: Color,
    /// 削除行の背景色。
    pub diff_del_bg: Color,
    /// 強調表示(単語単位)の追加箇所の、より明るい背景色。
    pub diff_add_bg_emphasis: Color,
    /// 強調表示(単語単位)の削除箇所の、より明るい背景色。
    pub diff_del_bg_emphasis: Color,
    /// diff ハンクのセクションヘッダ(関数名など)の色。muted より明るい。
    pub diff_section_header: Color,

    // 枠線
    /// パネルがフォーカスされている時の枠線色。
    pub border_focused: Color,
    /// パネルがフォーカスされていない時の枠線色。
    pub border_unfocused: Color,
    /// 補助的な枠線色(サブエリア間の区切り)。
    pub border_secondary: Color,

    // 選択
    /// 現在選択中の項目の背景色(アクティブなパネル)。
    pub selected_bg: Color,
    /// 現在選択中の項目の前景色(アクティブなパネル)。
    pub selected_fg: Color,
    /// 現在選択中の項目の背景色(非アクティブなパネル)。
    pub selected_bg_inactive: Color,
    /// 現在選択中の項目の前景色(非アクティブなパネル)。
    pub selected_fg_inactive: Color,

    // 行選択 (Viewer)
    /// ビューアで選択中の行の背景色。
    pub line_selected_bg: Color,
    /// ビューアで選択中の行の前景色。
    pub line_selected_fg: Color,

    // 行番号ガター
    /// 選択中の行のガター背景色。
    pub gutter_selected_bg: Color,
    /// 選択中の行のガター前景色。
    pub gutter_selected_fg: Color,
    /// ホバー時のガター行番号の前景色(muted よりやや明るい)。
    pub gutter_hover_fg: Color,
    /// ホバー時のガター行番号の背景色(クリック可能であることを示す控えめなハイライト)。
    pub gutter_hover_bg: Color,
    /// 保留中範囲の行のガター背景色(選択中より暗い)。
    pub gutter_pending_bg: Color,
    /// ビューアの保留中範囲行の背景色(選択中より暗い)。
    pub line_pending_bg: Color,

    // テキスト
    /// ヒント/補助テキストの色。
    pub hint: Color,
    /// 現在位置以外の検索マッチの前景色。
    pub search_match_fg: Color,
    /// 現在の検索マッチの背景色。
    pub search_match_bg: Color,
    /// 現在の検索マッチの前景色。
    pub search_current_fg: Color,

    // 待機中のパルス表示
    /// waiting インジケータの主色(明るいオレンジ)。
    pub waiting_primary: Color,
    /// waiting インジケータの副色(暗めのオレンジ)。
    pub waiting_secondary: Color,

    // タイトルバー
    /// タイトルバーの背景色。
    pub titlebar_bg: Color,
    /// タイトルバー内のディレクトリパス文字色。
    pub dir_fg: Color,

    // ステータスバーの背景
    /// 成功ステータスメッセージのフラッシュ背景色。
    pub status_bg_success: Color,
    /// エラーステータスメッセージのフラッシュ背景色。
    pub status_bg_error: Color,
    /// 警告ステータスメッセージのフラッシュ背景色。
    pub status_bg_warning: Color,
    /// 情報ステータスメッセージのフラッシュ背景色。
    pub status_bg_info: Color,

    // コメントオーバーレイ
    /// コメントプレビューポップアップの背景色。Claude が書いたコメント・返信の
    /// インラインスレッド面(既定の中立色)でもある。
    pub comment_preview_bg: Color,
    /// user が書いたコメント・返信のインラインスレッド面。comment_preview_bg とは
    /// 異なる色味にすることで、誰が書いたかを署名を読まずに一目で判別できるようにする。
    pub comment_user_bg: Color,
    /// 返信本文の文字色。
    pub reply_text: Color,

    // Markdown 表示
    /// Markdown(変更サマリやコメント本文)内の、コードブロックとインライン code の背景色。
    /// 背後に何が描画されていても GitHub 風に凹んで見えるよう、各テーマの基調色より
    /// 一段暗いカード状の色にしている。
    pub code_bg: Color,
    /// インライン code チップの前景色。見出し/アクセント色とは異なるソフトピンクにして、
    /// コード参照だと一目で分かるようにしている。
    pub code_fg: Color,

    // パネルの下地
    /// フォーカス中のリストパネル(worktree / explorer)の控えめな背景色。
    /// テーマ互換性のためフィールドとしては残しているが、layout.rs 側の面塗りは
    /// 透過化の整理で撤去済みで、代わりに端末の背景が透けて見える。
    #[allow(dead_code)]
    pub panel_focused_bg: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self::catppuccin_mocha()
    }
}
