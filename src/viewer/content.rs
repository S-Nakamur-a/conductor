//! ファイル内容の読み込み — ファイルを開いて content.file_content に格納する処理と、
//! それを支える小さなヘルパー群（メディア/markdown 判定、タブ展開）。

use std::fs;
use std::path::Path;

use crate::media_state;

use super::state::ViewerState;

impl ViewerState {
    /// diff の注釈キャッシュを無効化する（diff データが変わったときに呼ぶ）。
    pub fn invalidate_diff_annotations(&mut self) {
        self.content.cached_diff_annotations = None;
        self.content.cached_diff_annotations_file = None;
    }

    /// ファイルを開いて（読み込んで）、その行を file_content に格納する。
    ///
    /// relative_path は表示中のツリーの根からの相対。絶対パスに戻すのは
    /// [ViewerState::root] だけで、呼び出し側は根を知らなくてよい。
    pub fn open_file(&mut self, relative_path: &str, tab_width: usize) {
        self.exit_diff_mode();
        // md_rendered は意図的に維持する（このモードはファイルをまたいで持続する）。
        // スクロール位置は維持しない — 古いドキュメントを指す値になるため。
        self.md_scroll = 0;
        self.content.highlighted_lines.clear();
        self.content.highlighted_cache_key = None;
        self.content.grep_highlight_line = None;
        self.content.test_runs.clear();
        // 成功パスだけでなく先頭でクリアする: 下のメディア分岐や読み込みエラー分岐でも
        // file_content を差し替えるので、前のファイルのマスクが残っていると
        // 違うテキストを指したままになってしまう。
        self.content.code_mask = crate::symbol_index::CodeMask::default();
        // 前のファイルの失敗理由を持ち越さない。以降の分岐が必要なら再度立てる。
        self.content.load_error = None;
        let full = self.tree.root.join(relative_path);

        // メディアファイル（画像/動画）は aa-media 経由で扱う。
        if media_state::is_media_file(relative_path) {
            self.content.folds.clear();
            self.content.file_content.clear();
            self.content.current_file = Some(relative_path.to_string());
            self.content.file_scroll = 0;
            self.content.h_scroll = 0;
            // 実際の描画は render 時に遅延して起動する（パネルサイズが分かった時点で）。
            // 新しいファイル用に再描画されるようキャッシュをクリアする。
            self.media_state.clear();
            return;
        }

        // メディア以外のファイルを開くときは media state をクリアする。
        self.media_state.clear();

        match fs::read_to_string(&full) {
            Ok(text) => {
                self.content.file_content = text
                    .lines()
                    .map(|l| Self::expand_tabs(l, tab_width))
                    .collect();
                // ファイルが空行のみで長さゼロでない場合、空行を1行だけ表示する。
                if self.content.file_content.is_empty() && !text.is_empty() {
                    self.content.file_content.push(String::new());
                }
                self.content.current_file = Some(relative_path.to_string());
                self.content.file_scroll = 0;
                self.content.h_scroll = 0;
                // ジャンプ操作が可能になる前に、どの識別子がコードかを記録する。
                // file_content ではなく text から構築するのは、tree-sitter には
                // タブも含めて書かれたままのファイルが必要なため。
                self.content.code_mask =
                    crate::symbol_index::CodeMask::compute(&text, relative_path);
                // 折りたたみ範囲も展開前の text から求める（tree-sitter もインデント
                // 幅も、書かれたままのファイルを前提にしている）。同じファイルの
                // 再読み込みなら開閉は FoldState 側が引き継ぐ。
                self.content.folds.rebuild(&text, relative_path);
                // ▶ 実行ボタン向けに、実行可能なテストを検出する。言語ごとに振り分ける:
                // Go の *_test.go と Rust の *.rs。
                self.content.test_runs = if relative_path.ends_with(".rs") {
                    crate::rust_test::scan_rust_test_runs(&self.content.file_content, relative_path)
                } else {
                    crate::go_test::scan_go_test_runs(&self.content.file_content, relative_path)
                };
            }
            Err(e) => {
                // 失敗理由は専用のフィールドへ。以前はここで擬似的な 1 行を
                // file_content に流し込んでいたが、それだと行番号もハイライトも
                // 付いて本文と区別が付かず、Viewer 側からは「空 = 未選択」と
                // 見分けられなかった。
                log::warn!("failed to read {}: {e}", full.display());
                self.content.folds.clear();
                self.content.file_content.clear();
                self.content.load_error = Some(e.to_string());
                self.content.current_file = Some(relative_path.to_string());
                self.content.file_scroll = 0;
                self.content.h_scroll = 0;
            }
        }
    }

    /// 現在のファイルがメディアファイルなら true を返す。
    pub fn is_current_file_media(&self) -> bool {
        self.content
            .current_file
            .as_deref()
            .is_some_and(media_state::is_media_file)
    }

    /// Raw/Rendered トグルが、Viewer が今表示しているものに適用可能かどうか。
    ///
    /// markdown ファイルの素のファイル表示のときだけこれを提供する: unified-diff
    /// モードは diff を表示しており（そこで本文をレンダリングすると +/- の構造が
    /// 壊れる）、SUMMARY 疑似ファイルは定義上すでにレンダリング済み markdown である。
    /// この単一の述語がヘッダのトグルを描画するかどうかと、そのクリック対象が
    /// 有効かどうかの両方を決めるので、この2つがずれることはない。
    pub fn markdown_toggle_available(&self) -> bool {
        !self.show_summary
            && !self.diff_view.diff_mode
            && self
                .content
                .current_file
                .as_deref()
                .is_some_and(is_markdown_path)
    }

    /// Viewer が現在、生のソースの代わりにレンダリング済み markdown を描画しているか
    /// どうか。行に紐づく機能は全てこれで判定しなければならない: レンダリング済み
    /// 表示には行番号が無いので、行選択・ホバーハイライト・コメント作成/スレッド・
    /// 行に紐づくジャンプは、どこにも紐付けられない（ui::viewer_panel::markdown_view
    /// を参照）。
    pub fn is_showing_rendered_markdown(&self) -> bool {
        self.md_rendered && self.markdown_toggle_available()
    }

    /// 生のソースとレンダリング済み markdown を切り替える。レンダリング表示の
    /// スクロールをリセットし、切り替えると常にドキュメントの先頭に着地するようにする。
    ///
    /// あわせて、レンダリング表示では描画できない行に紐づくインタラクションを
    /// 破棄する。破棄しないと、選択範囲は戻ってきたときに黙って再出現してしまい、
    /// さらに悪いことに、開いたままのインラインリプライは画面上にもう無い
    /// コンポーズボックスへキー入力を飲み込み続けてしまう（トグルは開いている間も
    /// クリック可能なため）。
    pub fn toggle_markdown_rendered(&mut self) {
        self.md_rendered = !self.md_rendered;
        self.md_scroll = 0;
        self.clear_selection();
        // 進行中の gutter ドラッグをそのままにすると、mouse-up でコメント作成が
        // 開いてしまう。ドラッグ元の gutter が無い表示の上でそうなるのはおかしい。
        self.click.gutter_drag_anchor = None;
        self.explorer.inline_reply_line = None;
        self.explorer.inline_reply_comment_id = None;
        self.explorer.inline_reply_buffer.clear();
    }

    /// レンダリング済み markdown を離れ、行に紐づくジャンプ先が実際に見えるようにする。
    ///
    /// Viewer を行単位で位置づけるあらゆる経路から呼ばれる（定義へジャンプ、
    /// ジャンプ履歴、grep のヒット、ターミナルからの file:line）。これが無いと、
    /// これらのジャンプは行番号の無いレンダリング済み本文へ着地してしまう —
    /// 要求された行は黙って無視され、読み手はドキュメントの先頭に落とされてしまう。
    pub fn show_raw_for_line_target(&mut self) {
        self.md_rendered = false;
    }

    /// タブ文字をタブストップ位置に従ってスペースへ展開する。
    fn expand_tabs(line: &str, tab_width: usize) -> String {
        if !line.contains('\t') {
            return line.to_string();
        }
        let mut result = String::with_capacity(line.len());
        let mut col = 0;
        for ch in line.chars() {
            if ch == '\t' {
                let spaces = tab_width - (col % tab_width);
                for _ in 0..spaces {
                    result.push(' ');
                }
                col += spaces;
            } else {
                result.push(ch);
                col += 1;
            }
        }
        result
    }
}

/// path が markdown ファイルを指しているかどうか、すなわち Viewer が
/// Raw/Rendered トグルを提供できるファイルかどうか。
///
/// 拡張子のみで判定し、大文字小文字を区別しない（README.MD も該当する）。
/// .mdx、.mdown、拡張子なしの README には意図的に一致させない: レンダラは
/// change summary で使う小さな CommonMark のサブセットなので、対象を広げると
/// 表現できないファイルを黙って誤整形してしまう。
pub fn is_markdown_path(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ファイルの中身はワークツリーの実ファイルから読む。git を一切経由しないので
    /// .git の無いディレクトリでも、git 管理下の未追跡・未コミットのファイルでも
    /// 同じように開ける。これが崩れると「git 管理外だと Viewer が空のまま」に戻る。
    #[test]
    fn open_file_reads_from_disk_without_git() {
        let dir = std::env::temp_dir().join(format!("nogit_open_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("plain.txt"), "ALPHA\nBRAVO\n").unwrap();
        assert!(!dir.join(".git").exists(), "fixture must not be a git repo");

        let mut vs = ViewerState::default();

        vs.set_root(dir.clone());
        vs.open_file("plain.txt", 4);

        assert_eq!(vs.content.file_content, vec!["ALPHA", "BRAVO"]);
        assert_eq!(vs.content.current_file.as_deref(), Some("plain.txt"));
        assert!(vs.content.load_error.is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 読み込みに失敗したら理由を load_error に残す。本文が空になるのは
    /// 「未選択」「空ファイル」も同じなので、失敗をここで区別できないと
    /// Viewer が「ファイル未選択」に丸めてしまう。
    #[test]
    fn open_file_records_why_it_failed() {
        let dir = std::env::temp_dir().join(format!("nogit_err_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let mut vs = ViewerState::default();

        vs.set_root(dir.clone());
        vs.open_file("missing.txt", 4);

        assert!(vs.content.file_content.is_empty());
        assert_eq!(vs.content.current_file.as_deref(), Some("missing.txt"));
        assert!(
            vs.content.load_error.is_some(),
            "a failed read must record its reason, not look like 'nothing selected'"
        );

        // 次に成功したら理由は消える。持ち越すと直後の正常なファイルまで
        // エラー表示になる。
        std::fs::write(dir.join("ok.txt"), "OK\n").unwrap();
        vs.open_file("ok.txt", 4);
        assert!(vs.content.load_error.is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ファイルを開いたら、viewer が描画するタブ展開後の行と整合するマスクが
    /// 残らなければならない。ナビゲーションのクエリは全てその展開後の位置を
    /// 参照するため。CodeMask::compute ではなく open_file を通して駆動することで、
    /// 展開とマスクを互いに突き合わせて検証する。fixture がタブインデントに
    /// なっているのはまさにそのため。
    #[test]
    fn opening_a_file_masks_its_comments_and_strings() {
        let dir = std::env::temp_dir().join(format!("mask_open_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("sample.go"),
            "package main\n// Server handles things\nfunc Serve() {\n\tname := \"Server\"\n}\n",
        )
        .unwrap();

        let mut vs = ViewerState::default();

        vs.set_root(dir.clone());
        vs.open_file("sample.go", 4);

        // build_symbol_hints と同じやり方で各行を走査し、ジャンプ可能として
        // 提示するはずの単語を集める。
        let mut jumpable: Vec<(usize, String)> = Vec::new();
        for (i, line) in vs.content.file_content.iter().enumerate() {
            let line_1 = i + 1;
            for (k, (_, _, w)) in crate::symbol_index::identifier_occurrences(line).enumerate() {
                if vs.content.code_mask.is_code(line_1, k) {
                    jumpable.push((line_1, w.to_string()));
                }
            }
        }

        // "Server" は3回現れる: コメント中、コード中で関数の隣、文字列中。
        // 残るのはコード中のものだけ。
        let servers: Vec<usize> = jumpable
            .iter()
            .filter(|(_, w)| w == "Server")
            .map(|(line, _)| *line)
            .collect();
        assert!(
            servers.is_empty(),
            "comment and string occurrences of `Server` must not be jumpable, got lines {servers:?}"
        );

        assert!(jumpable.contains(&(3, "func".to_string())));
        assert!(jumpable.contains(&(3, "Serve".to_string())));
        // タブインデントされた行でも解決できる。これはマスクを列ではなく
        // 出現位置で管理していることのポイント。
        assert!(jumpable.contains(&(4, "name".to_string())));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 文法を持たない言語のファイルは、前のファイルの結果を引き継いだり生の
    /// 単語一致にフォールバックしたりせず、何も提示しない。
    #[test]
    fn opening_an_unsupported_language_clears_the_mask() {
        let dir = std::env::temp_dir().join(format!("mask_unsup_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.rs"), "fn keep() {}\n").unwrap();
        std::fs::write(dir.join("b.py"), "def keep():\n    pass\n").unwrap();

        let mut vs = ViewerState::default();

        vs.set_root(dir.clone());
        vs.open_file("a.rs", 4);
        assert!(vs.content.code_mask.is_code(1, 0), "Rust file is masked");

        vs.open_file("b.py", 4);
        assert!(
            !vs.content.code_mask.is_code(1, 0),
            "Python must not inherit the Rust file's mask"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn markdown_paths_are_detected_case_insensitively() {
        assert!(is_markdown_path("README.md"));
        assert!(is_markdown_path("docs/plan.markdown"));
        assert!(is_markdown_path("README.MD"));
        assert!(is_markdown_path("a/b/c.Markdown"));
    }

    #[test]
    fn non_markdown_paths_are_rejected() {
        // トグルが付いてはならない近縁ケース: 別のレンダラ方言（.mdx）、拡張子なしの
        // ファイル、そして単に名前に "md" が含まれるだけのもの。
        assert!(!is_markdown_path("src/main.rs"));
        assert!(!is_markdown_path("page.mdx"));
        assert!(!is_markdown_path("README"));
        assert!(!is_markdown_path("mdbook.toml"));
        assert!(!is_markdown_path(""));
    }

    /// このトグルは素のファイル表示のためのアフォーダンスである。diff モードと
    /// SUMMARY 疑似ファイルはそれぞれ独自のレンダラでパネル全体を占有するので、
    /// そこではトグルは消えなければならない — そして重要なのは、md_rendered が
    /// 保持されたままでも is_showing_rendered_markdown は一緒に false にならなければ
    /// ならないこと。さもないと diff 表示が、行に紐づく機能をオフにしたまま
    /// 描画されてしまう。
    #[test]
    fn rendered_markdown_is_confined_to_the_plain_file_view() {
        let mut vs = ViewerState::default();
        vs.content.current_file = Some("README.md".to_string());
        vs.md_rendered = true;
        assert!(vs.markdown_toggle_available());
        assert!(vs.is_showing_rendered_markdown());

        vs.diff_view.diff_mode = true;
        assert!(!vs.markdown_toggle_available());
        assert!(!vs.is_showing_rendered_markdown());
        vs.diff_view.diff_mode = false;

        vs.show_summary = true;
        assert!(!vs.markdown_toggle_available());
        assert!(!vs.is_showing_rendered_markdown());
        vs.show_summary = false;

        vs.content.current_file = Some("src/main.rs".to_string());
        assert!(!vs.markdown_toggle_available());
        assert!(!vs.is_showing_rendered_markdown());

        // これらすべてを経てもモードは保持されたままなので、素の表示で markdown
        // ファイルへ戻るとレンダリングが再開する（セッション内で持続する）。
        vs.content.current_file = Some("CHANGELOG.md".to_string());
        assert!(vs.is_showing_rendered_markdown());
    }

    /// 選択範囲や開いたままのインラインリプライは raw 表示に属するもの。
    /// どちらもレンダリング済み表示へ持ち越すと問題が黙って起きる: リプライ
    /// ボックスは描画されなくなってもキー入力を奪い続け、選択範囲は戻ったときに
    /// 再出現する。
    #[test]
    fn toggling_tears_down_line_anchored_interactions() {
        let mut vs = ViewerState {
            selection: crate::viewer::LineSelection::Selected { start: 3, end: 9 },
            ..Default::default()
        };
        vs.explorer.inline_reply_line = Some(7);
        vs.explorer.inline_reply_comment_id = Some("c1".to_string());

        vs.toggle_markdown_rendered();

        assert_eq!(vs.selection, crate::viewer::LineSelection::None);
        assert_eq!(vs.explorer.inline_reply_line, None);
        assert_eq!(vs.explorer.inline_reply_comment_id, None);
    }

    /// file:line へのジャンプはその行を見せなければならない。レンダリング済み
    /// 本文には行が無いので、行に紐づく入口はまず raw 表示へ戻す。
    #[test]
    fn line_targets_drop_out_of_rendered_mode() {
        let mut vs = ViewerState {
            md_rendered: true,
            ..Default::default()
        };
        vs.content.current_file = Some("README.md".to_string());
        assert!(vs.is_showing_rendered_markdown());
        vs.show_raw_for_line_target();
        assert!(!vs.is_showing_rendered_markdown());
        assert!(!vs.md_rendered);
    }

    #[test]
    fn toggling_resets_the_rendered_scroll() {
        let mut vs = ViewerState {
            md_scroll: 42,
            ..Default::default()
        };
        vs.toggle_markdown_rendered();
        assert!(vs.md_rendered);
        assert_eq!(vs.md_scroll, 0);
    }
}
