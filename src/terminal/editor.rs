//! 埋め込みエディタパネル: マージされたExplorer+Viewer領域を占有するPTYで
//! $VISUAL/$EDITORを起動し、終了時に解体する。

use std::path::PathBuf;

use crate::app::{App, StatusLevel};
use crate::types::Focus;

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
    pub cache: crate::terminal::render::pty::PtyRenderCache,
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
        let Some(path) = editor_target(self.viewer.content.current_file.as_deref(), &working_dir)
        else {
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
                self.request_redraw();
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
        self.request_redraw();
        let fname = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        self.set_status(format!("Edited {fname}"), StatusLevel::Success);
    }

    /// [Self::exit_editor] (再読み込みとステータスを足す) と worktree 切り替え
    /// (文脈がどのみち再読み込みされるので黙って捨てる) が共有する中核。
    fn take_down_editor(&mut self) -> Option<PathBuf> {
        let panel = self.editor.take()?;
        // エディタのPTYを削除する（子プロセスがすでに終了していてもkillは
        // 無害）。他のセッションのインデックスも調整する。
        self.close_terminal_session(panel.session_idx);
        // set_focus を通さないのは、再読み込みの主導権を呼び出し側に残すため。
        // Claude へ移っていてエディタが足元で終了した場合はそのままにする。
        if self.focus.current() == Focus::Editor {
            self.focus.enter(Focus::Viewer);
        }
        if self.layout.expanded == Some(Focus::Editor) {
            self.layout.expanded = None;
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
    pub(crate) fn editor_pty_size(&self) -> (u16, u16) {
        let cols = &self.layout.cache.columns;
        let region_w = cols[1].width.saturating_add(cols[2].width);
        let region_h = cols[1].height;
        let expanded = self.layout.expanded == Some(Focus::Editor);
        editor_content_size(region_w, region_h, expanded)
    }
}

/// None は「編集対象なし」。呼び出し側は不正な対象でエディタを起動する代わりに
/// ヒントを出す。
fn editor_target(current_file: Option<&str>, worktree_root: &std::path::Path) -> Option<PathBuf> {
    let rel = current_file?;
    if rel.is_empty() {
        return None;
    }
    Some(worktree_root.join(rel))
}

/// ゼロのリージョン (レイアウト未計算) には妥当な既定を返す。毎フレームの
/// sync_pty_sizes が後で補正する。vt100 が要るので 0 は返さない。
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

/// 空白のみの値は無視する。意図しない EDITOR="" が空のコマンドを生まないため。
/// パス自体に空白を含むエディタは非対応 (シェル風のクォート解釈はしない)。
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
    fn エディタの対象はworktree基準で相対を解決する() {
        let root = std::path::Path::new("/repo/wt");
        assert_eq!(
            editor_target(Some("src/main.rs"), root),
            Some(PathBuf::from("/repo/wt/src/main.rs"))
        );
    }

    #[test]
    fn ファイルが開いていなければ対象は無い() {
        // 決め手となる分岐: 開いているファイルが無ければ → エディタは起動しない。
        assert_eq!(editor_target(None, std::path::Path::new("/repo/wt")), None);
    }

    #[test]
    fn 空のパスなら対象は無い() {
        assert_eq!(
            editor_target(Some(""), std::path::Path::new("/repo/wt")),
            None
        );
    }
    /// VISUAL > EDITOR > fallback の順。空白のみの値は空のコマンドを生まないよう飛ばす。
    /// 分割は素朴で、シェル風のクォート解釈は行わない (意図的な制限)。
    #[test]
    fn エディタのコマンドは優先順で選び素朴に分割する() {
        let cases: [(Option<&str>, Option<&str>, Vec<&str>); 9] = [
            (None, None, vec!["vi"]),
            (Some("vim"), Some("nano"), vec!["vim"]),
            (None, Some("nano"), vec!["nano"]),
            (Some("code -w"), None, vec!["code", "-w"]),
            (Some("code\t-w  -n"), None, vec!["code", "-w", "-n"]),
            (Some(""), None, vec!["vi"]),
            (Some("   "), None, vec!["vi"]),
            (Some(""), Some("nano"), vec!["nano"]),
            (
                Some("vim -c 'set ft=rust'"),
                None,
                vec!["vim", "-c", "'set", "ft=rust'"],
            ),
        ];
        for (visual, editor, want) in cases {
            assert_eq!(
                resolve_editor_command(visual, editor, "vi"),
                want,
                "visual={visual:?} editor={editor:?}"
            );
        }
        assert_eq!(
            resolve_editor_command(Some("  vim  "), None, "vi"),
            vec!["vim"]
        );
    }

    #[test]
    fn エディタの中身の大きさは枠を引く() {
        // 非最大化: タイトル行＋下境界（2行）と左右境界（2列）。
        assert_eq!(editor_content_size(80, 40, false), (38, 78));
        // 最大化: タイトル行のみで境界線は無い。
        assert_eq!(editor_content_size(80, 40, true), (39, 80));
    }

    #[test]
    fn 領域が0なら既定の大きさになる() {
        assert_eq!(editor_content_size(0, 40, false), (24, 80));
        assert_eq!(editor_content_size(80, 0, false), (24, 80));
    }

    #[test]
    fn 中身の大きさが0になることはない() {
        // 極小のリージョンはアンダーフローせず1×1にクランプされる（vt100は≥1が必要）。
        for w in 1..=3u16 {
            for h in 1..=3u16 {
                let (rows, c) = editor_content_size(w, h, false);
                assert!(rows >= 1 && c >= 1, "w={w} h={h} → ({rows},{c})");
            }
        }
    }
}
