//! 埋め込みエディタパネル: マージされたExplorer+Viewer領域を占有するPTYで
//! $VISUAL/$EDITORを起動し、終了時に解体する。

use std::path::PathBuf;

use super::focus::Focus;
use super::{App, StatusLevel};

/// 稼働中の埋め込みエディタパネル（PTY内のvim/emacs）の状態。
///
/// 一時的な存在: ユーザーが$EDITORでファイルを開いたときに作られ、エディタ
/// プロセスが終了したときに破棄される。描画キャッシュを自前で持つので、
/// エディタパネルはClaude/Shellターミナルのキャッシュとは独立に描画される。
pub struct EditorPanel {
    /// PtyManagerのセッション一覧内での、エディタのPTYセッションのインデックス。
    /// 他のセッションが削除されたときに（ずれた分を補正して/クリアして）
    /// 同期を保つ。
    pub session_idx: usize,
    /// 編集中のファイルの絶対パス — 終了時の再読み込みとパネルタイトルに使う。
    pub path: PathBuf,
    /// エディタパネル用にキャッシュされたPTY描画出力（TerminalState内の
    /// Claude/Shellキャッシュに相当）。
    pub cache: crate::ui::common::PtyRenderCache,
    /// PTYリーダースレッドが再描画すべき新しい出力を出したときにセットされる。
    pub dirty: bool,
}

impl App {
    /// 現在Viewerに表示されているファイルを埋め込みエディタパネルで開く
    /// （マージされたExplorer+Viewer領域を占有するPTY内の$VISUAL/$EDITOR）。
    /// viewerの相対パスcurrent_fileを選択中のworktreeに対して解決する。
    /// 開いているファイルが無ければ代わりにヒントを表示する。エディタが
    /// すでに開いている場合は何もしない。
    pub fn open_in_editor(&mut self) {
        if self.editor.is_some() {
            return;
        }
        // grabされたworktreeのターミナルはロックされている（そのセッションは
        // main側で動く）ので、ここでエディタを開くとフリーズしてしまう。
        // 操作不能なエディタにユーザーを閉じ込めるより、拒否した方がよい。
        if self.is_selected_worktree_grabbed() {
            self.set_status(
                "Cannot edit while this worktree is grabbed".to_string(),
                StatusLevel::Warning,
            );
            return;
        }
        let (worktree_name, working_dir) = self.selected_worktree_info();
        let Some(path) = editor_target(
            self.viewer_state.content.current_file.as_deref(),
            &working_dir,
        ) else {
            self.set_status("No file open to edit".to_string(), StatusLevel::Warning);
            return;
        };

        let argv = resolve_editor_command(
            std::env::var("VISUAL").ok().as_deref(),
            std::env::var("EDITOR").ok().as_deref(),
            "vi",
        );
        // resolve_editor_commandは空のvecを返すことはない。
        let (program, args) = argv.split_first().expect("editor command is non-empty");

        let (rows, cols) = self.editor_pty_size();
        match self.terminal.pty_manager.spawn_editor_session(
            &worktree_name,
            "editor",
            &working_dir,
            rows,
            cols,
            program,
            args,
            &path,
        ) {
            Ok(idx) => {
                self.terminal.pty_manager.activate_session(idx);
                let fname = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string());
                self.editor = Some(EditorPanel {
                    session_idx: idx,
                    path,
                    cache: Default::default(),
                    dirty: true,
                });
                self.set_focus(Focus::Editor);
                // 置き換えるパネルの上にエディタの代替スクリーンがきれいに
                // 描画されるよう、ゼロから再描画する。
                self.terminal.needs_clear = true;
                self.dirty.mark_all();
                self.set_status(
                    format!("Editing {fname} — Ctrl+Esc: Claude · :q: close · ctrl+alt+z: zoom"),
                    StatusLevel::Info,
                );
            }
            Err(e) => {
                self.set_status(format!("Failed to launch editor: {e}"), StatusLevel::Error);
            }
        }
    }

    /// 埋め込みエディタパネルを解体する: PTYセッションをkill/削除し、
    /// フォーカスをViewerへ戻し、変更が即座に見えるよう編集直後のファイルを
    /// 再読み込みする（デバウンスされたファイルウォッチャーのリフレッシュと
    /// 対をなす）。
    pub fn exit_editor(&mut self) {
        let Some(path) = self.take_down_editor() else {
            return;
        };
        // 編集直後のファイルを即座に再読み込みする（ファイルウォッチャーの対と
        // 同じ扱い）。
        self.refresh_viewer();
        self.refresh_diff();
        self.dirty.mark_all();
        let fname = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        self.set_status(format!("Edited {fname}"), StatusLevel::Success);
    }

    /// エディタのPTYを解体してパネルを破棄し、編集していたパスを返す
    /// （エディタが開いていなければNone）。[Self::exit_editor]
    /// （再読み込みとステータス表示を追加する）とworktree切り替え
    /// （周囲の文脈がどのみち再読み込みされるので、エディタを黙って破棄する）
    /// が共有する中核部分。
    fn take_down_editor(&mut self) -> Option<PathBuf> {
        let panel = self.editor.take()?;
        // エディタのPTYを削除する（子プロセスがすでに終了していてもkillは
        // 無害）。他のセッションのインデックスも調整する。
        self.close_terminal_session(panel.session_idx);
        // フォーカスが（今は無くなった）エディタにあった場合のみViewerへ
        // 移す — これが通常の:qの流れ。ユーザーがClaudeへ移っていて、
        // エディタが足元で終了した場合はフォーカスをそのままにする。
        // 「エディタが最大化されている」という古い状態だけを落とす。
        // （set_focus経由ではなく）直接代入することで、呼び出し側が
        // 再読み込みの制御権を持つ。
        if self.focus == Focus::Editor {
            self.focus = Focus::Viewer;
        }
        if self.expanded_panel == Some(Focus::Editor) {
            self.expanded_panel = None;
        }
        self.terminal.needs_clear = true;
        Some(panel.path)
    }

    /// 所属するworktreeから離れるときにエディタパネルを破棄する。
    /// 再読み込みや通知はしない — 呼び出し側（[on_worktree_changed]）が
    /// どのみち新しいworktreeのビューを再読み込みするため。
    pub fn discard_editor_on_worktree_change(&mut self) {
        self.take_down_editor();
    }

    /// 埋め込みエディタが開いていて、そのプロセスが終了していれば
    /// （例: :q）解体し、通常のレイアウトへ戻す。閉じた場合はtrueを返す。
    /// メインループの反復ごとに呼ばれるので、遅い停止セッション掃除
    /// タイマーを待つのではなく、パネルは速やかに消える。
    pub fn poll_editor_exit(&mut self) -> bool {
        let Some(idx) = self.editor.as_ref().map(|e| e.session_idx) else {
            return false;
        };
        if self.terminal.pty_manager.is_session_alive(idx) {
            return false;
        }
        self.exit_editor();
        true
    }

    /// キャッシュされたレイアウトから、エディタPTYのコンテンツサイズ
    /// (rows, cols)を計算する: エディタはマージされたExplorer+Viewer領域を
    /// 占有し、タイトル行と境界線（パネル最大化時は消える）を差し引く。
    pub(super) fn editor_pty_size(&self) -> (u16, u16) {
        let cols = &self.layout.cache.columns;
        let region_w = cols[1].width.saturating_add(cols[2].width);
        let region_h = cols[1].height;
        let expanded = self.expanded_panel == Some(Focus::Editor);
        editor_content_size(region_w, region_h, expanded)
    }
}

/// viewerの相対パスcurrent_fileとworktreeの根から、外部エディタに渡す
/// 絶対パスを解決する。None（開いているファイルが無い、またはパスが空）は
/// 「編集対象なし」を意味し、呼び出し側は不正な対象に対してエディタを起動
/// する代わりにヒントを表示する。
fn editor_target(current_file: Option<&str>, worktree_root: &std::path::Path) -> Option<PathBuf> {
    let rel = current_file?;
    if rel.is_empty() {
        return None;
    }
    Some(worktree_root.join(rel))
}

/// 埋め込みエディタPTYのコンテンツサイズ (rows, cols) を、リージョンサイズと
/// 最大化状態から計算する。タイトル行は常に存在し、非最大化時はさらに下の
/// 境界行と左右の境界列を持つ。ゼロのリージョン（レイアウト未計算）には
/// 妥当なデフォルト値を与える — sync_pty_sizesでの毎フレームのリサイズが
/// 後で補正する。どちらの次元も0を返すことはない（vt100は最低1×1が必要）。
fn editor_content_size(region_w: u16, region_h: u16, expanded: bool) -> (u16, u16) {
    if region_w == 0 || region_h == 0 {
        return (24, 80);
    }
    let border_rows: u16 = if expanded { 1 } else { 2 };
    let border_cols: u16 = if expanded { 0 } else { 2 };
    (
        region_h.saturating_sub(border_rows).max(1),
        region_w.saturating_sub(border_cols).max(1),
    )
}

/// $VISUAL / $EDITORからエディタのコマンドラインを解決し、無ければfallback
/// にフォールバックする。空または空白のみの値は無視するので、意図しない
/// EDITOR=""が空のコマンドを生まない。選ばれた値は空白で区切ってプログラム
/// ＋引数に分割する（これで"code -w"のような指定が動く）。パスそのものに
/// 空白を含むエディタは意図的にサポートしない（シェル風のクォート解釈は
/// スコープ外）。
fn resolve_editor_command(
    visual: Option<&str>,
    editor: Option<&str>,
    fallback: &str,
) -> Vec<String> {
    let chosen = [visual, editor]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|s| !s.is_empty())
        .unwrap_or(fallback);
    let parts: Vec<String> = chosen.split_whitespace().map(str::to_string).collect();
    if parts.is_empty() {
        vec![fallback.to_string()]
    } else {
        parts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_target_resolves_relative_against_worktree() {
        let root = std::path::Path::new("/repo/wt");
        assert_eq!(
            editor_target(Some("src/main.rs"), root),
            Some(PathBuf::from("/repo/wt/src/main.rs"))
        );
    }

    #[test]
    fn editor_target_is_none_when_no_file_open() {
        // 決め手となる分岐: 開いているファイルが無ければ → エディタは起動しない。
        assert_eq!(editor_target(None, std::path::Path::new("/repo/wt")), None);
    }

    #[test]
    fn editor_target_is_none_for_empty_path() {
        assert_eq!(
            editor_target(Some(""), std::path::Path::new("/repo/wt")),
            None
        );
    }

    #[test]
    fn resolve_editor_falls_back_when_unset() {
        assert_eq!(resolve_editor_command(None, None, "vi"), vec!["vi"]);
    }

    #[test]
    fn resolve_editor_visual_takes_precedence() {
        assert_eq!(
            resolve_editor_command(Some("vim"), Some("nano"), "vi"),
            vec!["vim"]
        );
    }

    #[test]
    fn resolve_editor_uses_editor_when_visual_unset() {
        assert_eq!(
            resolve_editor_command(None, Some("nano"), "vi"),
            vec!["nano"]
        );
    }

    #[test]
    fn resolve_editor_splits_args() {
        assert_eq!(
            resolve_editor_command(Some("code -w"), None, "vi"),
            vec!["code", "-w"]
        );
        assert_eq!(
            resolve_editor_command(Some("code\t-w  -n"), None, "vi"),
            vec!["code", "-w", "-n"]
        );
    }

    #[test]
    fn resolve_editor_ignores_blank_values() {
        // 空白のみのVISUALはスキップされる。空のコマンドを生むのではなく、
        // EDITOR（またはfallback）が優先されるようにするため。
        assert_eq!(resolve_editor_command(Some(""), None, "vi"), vec!["vi"]);
        assert_eq!(resolve_editor_command(Some("   "), None, "vi"), vec!["vi"]);
        assert_eq!(
            resolve_editor_command(Some(""), Some("nano"), "vi"),
            vec!["nano"]
        );
        assert_eq!(
            resolve_editor_command(Some("  vim  "), None, "vi"),
            vec!["vim"]
        );
    }

    #[test]
    fn editor_content_size_subtracts_borders() {
        // 非最大化: タイトル行＋下境界（2行）と左右境界（2列）。
        assert_eq!(editor_content_size(80, 40, false), (38, 78));
        // 最大化: タイトル行のみで境界線は無い。
        assert_eq!(editor_content_size(80, 40, true), (39, 80));
    }

    #[test]
    fn editor_content_size_defaults_on_zero_region() {
        assert_eq!(editor_content_size(0, 40, false), (24, 80));
        assert_eq!(editor_content_size(80, 0, false), (24, 80));
    }

    #[test]
    fn editor_content_size_never_returns_zero() {
        // 極小のリージョンはアンダーフローせず1×1にクランプされる（vt100は≥1が必要）。
        for w in 1..=3u16 {
            for h in 1..=3u16 {
                let (rows, c) = editor_content_size(w, h, false);
                assert!(rows >= 1 && c >= 1, "w={w} h={h} → ({rows},{c})");
            }
        }
    }

    #[test]
    fn resolve_editor_naive_split_does_not_honor_quotes() {
        // 既知の制限: シェル風のクォート解釈は行わない。クォートされた引数も
        // 内部の空白で分割される。これは意図的な挙動を固定するテスト。
        assert_eq!(
            resolve_editor_command(Some("vim -c 'set ft=rust'"), None, "vi"),
            vec!["vim", "-c", "'set", "ft=rust'"]
        );
    }
}
