use super::*;
use conductor_core::diff_state::{DiffSource, FileDiff};
use conductor_core::keymap::Action;
use conductor_core::review_store::ReviewComment;
use conductor_svc::Services;
use crossterm::event::{KeyCode, KeyEvent};
use tempfile::TempDir;

use crate::effect::apply;
use crate::modal::Modal;
use crate::testing::pump;
use crate::workspace::{Focus, Workspace};

fn fixture(files: &[(&str, &str)]) -> TempDir {
    let dir = TempDir::new().unwrap();
    for (path, body) in files {
        std::fs::write(dir.path().join(path), body).unwrap();
    }
    dir
}

struct Harness {
    ws: Workspace,
    svc: Services<TaskResult>,
}

impl Harness {
    fn at(dir: &Path) -> Self {
        let mut harness = Self {
            ws: Workspace::for_test(),
            svc: Services::new(),
        };
        harness.ws.focus = Focus::Viewer;
        let effects = harness.ws.panels.viewer.set_root(dir.to_path_buf());
        harness.run(effects);
        harness
    }

    fn run(&mut self, effects: Vec<Effect>) {
        apply(&mut self.ws, &mut self.svc, effects);
        pump(&mut self.ws, &mut self.svc);
    }

    fn open(&mut self, path: &str) {
        let effects = self.viewer().open(Path::new(path), None, None, false);
        self.run(effects);
    }

    fn peek(&mut self, path: &str) {
        let effects = self.viewer().open(Path::new(path), None, None, true);
        self.run(effects);
    }

    fn act(&mut self, action: Action) {
        let effects = self.ws.dispatch(action).unwrap_or_default();
        self.run(effects);
    }

    fn viewer(&mut self) -> &mut ViewerPanel {
        &mut self.ws.panels.viewer
    }

    /// 2 打鍵目を実際の経路 (route) で送る。
    fn press(&mut self, code: KeyCode) {
        crate::run::on_key(&mut self.ws, &mut self.svc, KeyEvent::from(code));
        crate::testing::pump(&mut self.ws, &mut self.svc);
    }

    fn click(&mut self, x: u16, y: u16, extend: bool) -> Vec<Effect> {
        let root = self.ws.panels.viewer.root().to_path_buf();
        let (panels, _, ctx) = self.ws.split(&root);
        panels.viewer.click(x, y, extend, &ctx)
    }

    fn install(&mut self, comments: Vec<conductor_core::review_store::ReviewComment>) {
        self.ws.focus = Focus::Viewer;
        self.ws.review.install(Ok(crate::review::Snapshot {
            branch: "main".into(),
            comments,
            ..crate::review::Snapshot::default()
        }));
    }

    /// 描く直前の前処理を実際の経路で回す。
    fn prepare(&mut self) {
        let Workspace {
            panels,
            config,
            theme,
            ..
        } = &mut self.ws;
        let effects = panels.viewer.prepare(config, theme);
        self.run(effects);
    }

    /// レンダリング済み markdown の見えている行。
    fn rendered(&self) -> Vec<String> {
        self.ws
            .panels
            .viewer
            .content
            .rendered
            .iter()
            .skip(self.ws.panels.viewer.scroll.md)
            .map(|l| l.to_string())
            .collect()
    }

    fn body(&self) -> &[String] {
        &self.ws.panels.viewer.content.lines
    }

    fn tabs(&self) -> Vec<&str> {
        self.ws
            .panels
            .viewer
            .tabs()
            .iter()
            .map(|t| t.path.as_str())
            .collect()
    }
}

#[test]
fn 開くとタブが増え同じファイルは使い回す() {
    let dir = fixture(&[("a.txt", "A\n"), ("b.txt", "B\n")]);
    let mut h = Harness::at(dir.path());

    h.open("a.txt");
    h.open("b.txt");
    assert_eq!(h.tabs(), ["a.txt", "b.txt"]);

    h.open("a.txt");
    assert_eq!(h.tabs(), ["a.txt", "b.txt"], "既に開いているファイル");
    assert_eq!(h.ws.panels.viewer.active_path(), Some("a.txt"));
    assert_eq!(h.body(), ["A"]);
}

/// ツリーが畳まれたままだと、開いたファイルがどこにあるか分からない。
#[test]
fn ファイルを開くとツリーを追従させるeffectが出る() {
    let dir = fixture(&[("a.txt", "A\n")]);
    let mut h = Harness::at(dir.path());

    let effects = h.viewer().open(Path::new("a.txt"), None, None, false);
    let reveal = effects.iter().find_map(|e| match e {
        Effect::RevealInTree(path) => Some(path.as_str()),
        _ => None,
    });
    assert_eq!(reveal, Some("a.txt"), "{effects:?}");
}

/// タブごとに読んでいた位置を持つ。戻ったときに先頭へ巻き戻ると、差分レビュー中に
/// 行き来する用途では複数タブの意味が無くなる。
#[test]
fn タブを移ると読みかけの位置が戻りディスクから読み直す() {
    let long: String = (0..50).map(|i| format!("line{i}\n")).collect();
    let dir = fixture(&[("a.txt", long.as_str()), ("b.txt", long.as_str())]);
    let mut h = Harness::at(dir.path());

    h.open("a.txt");
    h.viewer().scroll.line = 30;
    h.open("b.txt");
    assert_eq!(h.ws.panels.viewer.scroll.line, 0, "新しいタブは先頭から");
    h.viewer().scroll.line = 10;

    std::fs::write(dir.path().join("a.txt"), "NEW\n").unwrap();
    h.act(Action::PrevViewerTab);
    assert_eq!(h.ws.panels.viewer.active_path(), Some("a.txt"));
    assert_eq!(h.body(), ["NEW"], "非アクティブ中の書き換えが反映される");

    h.act(Action::NextViewerTab);
    assert_eq!(h.ws.panels.viewer.active_path(), Some("b.txt"));
    assert_eq!(h.ws.panels.viewer.scroll.line, 10);
}

#[test]
fn タブを閉じると隣へ移り最後は未選択になる() {
    let dir = fixture(&[("a.txt", "A\n"), ("b.txt", "B\n")]);
    let mut h = Harness::at(dir.path());
    h.open("a.txt");
    h.open("b.txt");

    h.act(Action::CloseViewerTab);
    assert_eq!(h.tabs(), ["a.txt"]);
    assert_eq!(h.body(), ["A"]);

    h.act(Action::CloseViewerTab);
    assert!(h.tabs().is_empty());
    assert_eq!(h.ws.panels.viewer.content.path, None);
    assert!(h.body().is_empty());
}

/// クリックするたびにタブが増えるのを防ぐのが preview の本題。
#[test]
fn previewのタブは1枚だけで永続で開き直すと固定される() {
    let dir = fixture(&[("a.txt", "A\n"), ("b.txt", "B\n"), ("c.txt", "C\n")]);
    let mut h = Harness::at(dir.path());

    h.peek("a.txt");
    h.peek("b.txt");
    assert_eq!(h.tabs(), ["b.txt"], "preview は同時に 1 枚");
    assert_eq!(h.body(), ["B"]);

    // 永続で開くと残っていた preview は閉じる。
    h.open("c.txt");
    assert_eq!(h.tabs(), ["c.txt"]);
    assert!(!h.ws.panels.viewer.tabs()[0].status.is_preview());

    // 同じファイルを永続で開き直すと固定され、次を開いても残る。
    h.peek("a.txt");
    h.open("a.txt");
    h.peek("b.txt");
    assert_eq!(h.tabs(), ["c.txt", "a.txt", "b.txt"]);
}

#[test]
fn 別のタブへ移るとpreviewは閉じる() {
    let dir = fixture(&[("a.txt", "A\n"), ("b.txt", "B\n"), ("c.txt", "C\n")]);
    let mut h = Harness::at(dir.path());
    h.open("a.txt");
    h.open("b.txt");
    h.peek("c.txt");
    assert_eq!(h.tabs().len(), 3);

    let effects = h.viewer().focus_tab(0);
    h.run(effects);
    assert_eq!(h.tabs(), ["a.txt", "b.txt"]);
    assert_eq!(h.ws.panels.viewer.active_tab(), 0);
    assert_eq!(h.body(), ["A"]);
}

#[test]
fn 根が変わると無いファイルのタブは落ちる() {
    let a = fixture(&[("both.txt", "A\n"), ("only_a.txt", "A\n")]);
    let b = fixture(&[("both.txt", "B\n")]);
    let mut h = Harness::at(a.path());
    h.open("both.txt");
    h.open("only_a.txt");
    assert_eq!(h.tabs().len(), 2);

    let effects = h.viewer().set_root(b.path().to_path_buf());
    h.run(effects);
    assert_eq!(h.tabs(), ["both.txt"]);
    assert_eq!(h.body(), ["B"], "新しい根の中身を読む");
}

#[test]
fn 開けなかった理由を残す() {
    let dir = fixture(&[("ok.txt", "OK\n")]);
    let mut h = Harness::at(dir.path());

    h.open("missing.txt");
    assert!(h.body().is_empty());
    assert_eq!(
        h.ws.panels.viewer.content.path.as_deref(),
        Some("missing.txt")
    );
    assert!(h.ws.panels.viewer.content.error.is_some());

    // 持ち越すと直後の正常なファイルまでエラー表示になる。
    h.open("ok.txt");
    assert!(h.ws.panels.viewer.content.error.is_none());
}

#[test]
fn 遅れて届いた古い読み込みは捨てる() {
    let dir = fixture(&[("a.txt", "A\n"), ("b.txt", "B\n")]);
    let mut h = Harness::at(dir.path());
    h.open("a.txt");
    h.open("b.txt");

    let stale = TaskResult::FileLoaded {
        seq: 1,
        loaded: Ok(content::Loaded {
            lines: vec!["STALE".into()],
            folds: Vec::new(),
            mask: Default::default(),
            tests: Default::default(),
        }),
    };
    h.viewer().apply_result(stale);
    assert_eq!(h.body(), ["B"]);
}

#[test]
fn diffを添えて開くと差分になりescで素の本文へ戻る() {
    let dir = fixture(&[("a.txt", "one\ntwo\nthree\n")]);
    let mut h = Harness::at(dir.path());
    let file_diff = FileDiff {
        path: "a.txt".into(),
        added_lines: 1,
        deleted_lines: 0,
        hunks: vec![conductor_core::diff_state::DiffHunk {
            lines: vec![conductor_core::diff_state::DiffLine {
                tag: conductor_core::diff_state::DiffLineTag::Insert,
                old_line_no: None,
                new_line_no: Some(2),
                inline_segments: Vec::new(),
                content: "two".into(),
            }],
            func_header: None,
        }],
    };
    let effects = h.viewer().open(
        Path::new("a.txt"),
        None,
        Some(Box::new(OpenDiff {
            source: DiffSource::working_tree("main"),
            file: file_diff,
        })),
        false,
    );
    h.run(effects);

    assert!(h.ws.panels.viewer.diff.active);
    assert_eq!(h.ws.key_context(), KeyContext::ViewerDiffMode);
    h.act(Action::ExitToExplorer);
    assert!(
        !h.ws.panels.viewer.diff.active,
        "esc は先に diff から抜ける"
    );
    assert_eq!(h.ws.key_context(), KeyContext::Viewer);
    assert_eq!(h.body(), ["one", "two", "three"]);
}

#[test]
fn 検索は当たった行へ寄せて次と前に送る() {
    let dir = fixture(&[("a.txt", "alpha\nbeta\nalpha\n")]);
    let mut h = Harness::at(dir.path());
    h.open("a.txt");

    let effects = h.viewer().search_for("alpha");
    h.run(effects);
    assert_eq!(h.ws.panels.viewer.scroll.line, 0);
    h.act(Action::NextSearchMatch);
    assert_eq!(h.ws.panels.viewer.scroll.line, 2);
    h.act(Action::PrevSearchMatch);
    assert_eq!(h.ws.panels.viewer.scroll.line, 0);

    let effects = h.viewer().search_for("zzz");
    assert!(matches!(effects.as_slice(), [Effect::Status(..)]));
}

#[test]
fn 折りたたみの2打鍵目はパネルが直接読む() {
    let dir = fixture(&[("a.rs", "fn a() {\n    b();\n    c();\n}\n")]);
    let mut h = Harness::at(dir.path());
    h.open("a.rs");

    h.act(Action::FoldPrefix);
    assert!(h.ws.panels.viewer.awaiting_chord());
    h.viewer().scroll.line = 1;
    h.press(KeyCode::Char('c'));
    assert!(h.ws.panels.viewer.fold.is_collapsed(1));
    assert_eq!(
        h.ws.panels.viewer.scroll.line, 0,
        "隠れた行から見出しへ寄る"
    );
    assert!(!h.ws.panels.viewer.awaiting_chord());
}

/// 畳んだ行を飛ばして数えるので、画面 3 行目が 3 行目とは限らない。
#[test]
fn クリックした画面行はその位置の可視行を選ぶ() {
    let dir = fixture(&[("a.rs", "fn a() {\n    b();\n    c();\n}\nfn d() {}\n")]);
    let mut h = Harness::at(dir.path());
    h.open("a.rs");
    h.viewer().body = ratatui::layout::Rect::new(0, 10, 40, 20);

    h.click(20, 11, false);
    assert_eq!(h.ws.panels.viewer.selection.range(), Some((2, 2)));
    h.click(20, 13, true);
    assert_eq!(h.ws.panels.viewer.selection.range(), Some((2, 4)));

    h.viewer().fold.close(1);
    h.click(20, 11, false);
    assert_eq!(
        h.ws.panels.viewer.selection.range(),
        Some((5, 5)),
        "閉じ括弧まで畳むので 2..4 を飛ばす"
    );
}

/// ガターの桁ごとに意味が違う。印の下を押せば印の意味になる。
#[test]
fn ガターの桁は印と畳みと本文で意味が分かれる() {
    let comments = vec![crate::review::tests::comment("a", "a.rs", 2, None)];
    let dir = fixture(&[("a.rs", "fn a() {\n    b();\n    c();\n}\nfn d() {}\n")]);
    let mut h = Harness::at(dir.path());
    h.open("a.rs");
    h.install(comments.clone());
    h.viewer().body = ratatui::layout::Rect::new(0, 10, 40, 20);
    let all: Vec<&conductor_core::review_store::ReviewComment> = comments.iter().collect();

    // 印は 0..2、行番号は 2、その右の 3 が畳みの印、以降が本文。
    assert!(h.ws.panels.viewer.threads.is_open(&all, 2));
    h.click(0, 11, false);
    assert!(
        !h.ws.panels.viewer.threads.is_open(&all, 2),
        "印でスレッドを畳む"
    );
    assert!(h.ws.panels.viewer.selection.is_empty());

    h.click(3, 10, false);
    assert!(h.ws.panels.viewer.fold.is_collapsed(1), "畳みの印で畳む");
    assert!(h.ws.panels.viewer.selection.is_empty());

    h.click(20, 10, false);
    assert_eq!(h.ws.panels.viewer.selection.range(), Some((1, 1)));
}

/// キーの c を知らなくてもコメントを始められる経路。
#[test]
fn ガターを押すとコメントの作成欄が開く() {
    let dir = fixture(&[("a.txt", "one\ntwo\nthree\n")]);
    let mut h = Harness::at(dir.path());
    h.open("a.txt");
    h.viewer().body = ratatui::layout::Rect::new(0, 10, 40, 20);

    let effects = h.click(0, 11, false);
    let [Effect::PushModal(Modal::CommentEditor(editor))] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert!(
        matches!(
            editor.target,
            crate::modal::EditTarget::New {
                line_start: 2,
                line_end: None,
                ..
            }
        ),
        "{:?}",
        editor.target
    );
}

/// 行番号の桁は既にコメントのある行でも作成を始めるので、重なった範囲も作れる。
#[test]
fn コメントのある行の印はスレッドを開閉し行番号の桁は作成を始める() {
    let comments = vec![crate::review::tests::comment("a", "a.txt", 2, None)];
    let dir = fixture(&[("a.txt", "one\ntwo\nthree\n")]);
    let mut h = Harness::at(dir.path());
    h.open("a.txt");
    h.install(comments.clone());
    h.viewer().body = ratatui::layout::Rect::new(0, 10, 40, 20);
    let all: Vec<&ReviewComment> = comments.iter().collect();

    assert!(h.ws.panels.viewer.threads.is_open(&all, 2));
    assert!(h.click(0, 11, false).is_empty(), "作成へは行かない");
    assert!(!h.ws.panels.viewer.threads.is_open(&all, 2));

    let effects = h.click(2, 11, false);
    assert!(
        matches!(
            effects.as_slice(),
            [Effect::PushModal(Modal::CommentEditor(_))]
        ),
        "{effects:?}"
    );
}

#[test]
fn 開いたスレッドの下の行を押すとその行が対象になる() {
    let dir = fixture(&[("a.txt", "one\ntwo\nthree\n")]);
    let mut h = Harness::at(dir.path());
    h.open("a.txt");
    h.install(vec![crate::review::tests::comment("a", "a.txt", 1, None)]);
    h.viewer().body = ratatui::layout::Rect::new(0, 10, 40, 20);

    let drawn = render::body(
        &h.ws.panels.viewer,
        &h.ws.review,
        &h.ws.theme,
        h.ws.config.ui.icon_set(),
        40,
        20,
    );
    let offset = drawn
        .iter()
        .position(|line| line.to_string().contains("two"))
        .expect("2 行目");
    assert!(offset > 1, "スレッドが割り込んでいる: {offset}");

    let effects = h.click(2, 10 + offset as u16, false);
    let [Effect::PushModal(Modal::CommentEditor(editor))] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert!(
        matches!(
            editor.target,
            crate::modal::EditTarget::New { line_start: 2, .. }
        ),
        "{:?}",
        editor.target
    );
}

/// 差分を読みながらコメントを付けるのが主な使い方。
#[test]
fn 差分表示でもガターから作成でき削除行では断る() {
    let dir = fixture(&[("a.txt", "one\n")]);
    let mut h = Harness::at(dir.path());
    let file_diff = FileDiff {
        path: "a.txt".into(),
        added_lines: 1,
        deleted_lines: 1,
        hunks: vec![conductor_core::diff_state::DiffHunk {
            lines: vec![
                conductor_core::diff_state::DiffLine {
                    tag: conductor_core::diff_state::DiffLineTag::Delete,
                    old_line_no: Some(1),
                    new_line_no: None,
                    inline_segments: Vec::new(),
                    content: "gone".into(),
                },
                conductor_core::diff_state::DiffLine {
                    tag: conductor_core::diff_state::DiffLineTag::Insert,
                    old_line_no: None,
                    new_line_no: Some(1),
                    inline_segments: Vec::new(),
                    content: "one".into(),
                },
            ],
            func_header: None,
        }],
    };
    let effects = h.viewer().open(
        Path::new("a.txt"),
        None,
        Some(Box::new(OpenDiff {
            source: DiffSource::working_tree("main"),
            file: file_diff,
        })),
        false,
    );
    h.run(effects);
    h.viewer().body = ratatui::layout::Rect::new(0, 10, 40, 20);

    let effects = h.click(2, 11, false);
    let [Effect::PushModal(Modal::CommentEditor(editor))] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert!(
        matches!(
            editor.target,
            crate::modal::EditTarget::New { line_start: 1, .. }
        ),
        "{:?}",
        editor.target
    );

    let effects = h.click(2, 10, false);
    assert!(
        matches!(
            effects.as_slice(),
            [Effect::Status(StatusLevel::Warning, _)]
        ),
        "{effects:?}"
    );
    assert!(h.click(20, 10, false).is_empty(), "本文の桁は断らない");
}

#[test]
fn 差分の隠れた塊を押すと展開される() {
    let dir = fixture(&[("a.txt", "one\ntwo\nthree\nfour\nfive\nsix\nseven\n")]);
    let mut h = Harness::at(dir.path());
    let line = |no| conductor_core::diff_state::DiffLine {
        tag: conductor_core::diff_state::DiffLineTag::Equal,
        old_line_no: Some(no),
        new_line_no: Some(no),
        inline_segments: Vec::new(),
        content: "".into(),
    };
    let file_diff = FileDiff {
        path: "a.txt".into(),
        added_lines: 0,
        deleted_lines: 0,
        hunks: vec![
            conductor_core::diff_state::DiffHunk {
                lines: vec![line(1)],
                func_header: None,
            },
            conductor_core::diff_state::DiffHunk {
                lines: vec![line(7)],
                func_header: None,
            },
        ],
    };
    let effects = h.viewer().open(
        Path::new("a.txt"),
        None,
        Some(Box::new(OpenDiff {
            source: DiffSource::working_tree("main"),
            file: file_diff,
        })),
        false,
    );
    h.run(effects);
    h.viewer().body = ratatui::layout::Rect::new(0, 10, 40, 20);
    assert!(matches!(
        h.ws.panels.viewer.diff.entries[1],
        diff::Entry::ExpandableContext { .. }
    ));

    let effects = h.click(20, 11, false);
    assert!(effects.is_empty(), "コメント作成には行かない: {effects:?}");
    assert!(
        !h.ws
            .panels
            .viewer
            .diff
            .entries
            .iter()
            .any(|e| matches!(e, diff::Entry::ExpandableContext { .. })),
        "{:?}",
        h.ws.panels.viewer.diff.entries
    );
}

#[test]
fn ホイールは畳みを跨いで送り差分では行ではなくエントリを送る() {
    let dir = fixture(&[("a.rs", "fn a() {\n    b();\n    c();\n}\nfn d() {}\n")]);
    let mut h = Harness::at(dir.path());
    h.open("a.rs");
    h.viewer().fold.close(1);
    h.viewer().scroll_lines(1);
    assert_eq!(h.ws.panels.viewer.scroll.line, 4);
    h.viewer().scroll_lines(-1);
    assert_eq!(h.ws.panels.viewer.scroll.line, 0);
}

/// zm / zr は何段畳んだかを返す。押した結果が画面に出ないと、どこまで畳んだのか
/// 分からないまま連打することになる。
#[test]
fn 深さ単位の畳みは段数をステータスに出す() {
    let dir = fixture(&[("a.rs", "fn a() {\n    if x {\n        y();\n    }\n}\n")]);
    let mut h = Harness::at(dir.path());
    h.open("a.rs");

    let effects = h.viewer().fold_chord('m');
    let [Effect::Status(StatusLevel::Info, text)] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert_eq!(text, "fold level 1/2");

    let effects = h.viewer().fold_chord('a');
    assert!(effects.is_empty(), "行単位の畳みは段数を出さない");
}

/// 選択があれば範囲、無ければカーソル行。どちらもコメント側の座標で渡す。
#[test]
fn cは選択の範囲をそのままコメントのアンカーにする() {
    let dir = fixture(&[("a.txt", "one\ntwo\nthree\nfour\n")]);
    let mut h = Harness::at(dir.path());
    h.open("a.txt");
    h.ws.focus = Focus::Viewer;

    h.viewer().scroll.line = 2;
    let effects = h.ws.dispatch(Action::AddComment).unwrap();
    let [Effect::PushModal(Modal::CommentEditor(editor))] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert!(matches!(
        editor.target,
        crate::modal::EditTarget::New {
            line_start: 3,
            line_end: None,
            ..
        }
    ));

    h.viewer().selection.click(2, false);
    h.viewer().selection.click(4, true);
    let effects = h.ws.dispatch(Action::AddComment).unwrap();
    let [Effect::PushModal(Modal::CommentEditor(editor))] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert!(matches!(
        editor.target,
        crate::modal::EditTarget::New {
            line_start: 2,
            line_end: Some(4),
            ..
        }
    ));
    assert!(
        h.ws.panels.viewer.selection.is_empty(),
        "書き始めたら選択は畳む"
    );
}

/// コメントの座標は新ファイル側の行番号なので、削除行には置き場所が無い。
#[test]
fn 削除行ではコメントを始められない() {
    let dir = fixture(&[("a.txt", "one\n")]);
    let mut h = Harness::at(dir.path());
    let file_diff = FileDiff {
        path: "a.txt".into(),
        added_lines: 0,
        deleted_lines: 1,
        hunks: vec![conductor_core::diff_state::DiffHunk {
            lines: vec![conductor_core::diff_state::DiffLine {
                tag: conductor_core::diff_state::DiffLineTag::Delete,
                old_line_no: Some(1),
                new_line_no: None,
                inline_segments: Vec::new(),
                content: "gone".into(),
            }],
            func_header: None,
        }],
    };
    let effects = h.viewer().open(
        Path::new("a.txt"),
        None,
        Some(Box::new(OpenDiff {
            source: DiffSource::working_tree("main"),
            file: file_diff,
        })),
        false,
    );
    h.run(effects);
    h.ws.focus = Focus::Viewer;

    let entry = h.ws.panels.viewer.diff.entries.iter().position(|e| {
        matches!(
            e,
            diff::Entry::Line {
                new_line_no: None,
                ..
            }
        )
    });
    h.viewer().scroll.diff = entry.expect("削除行");
    assert_eq!(h.ws.panels.viewer.comment_line(), None);
    let effects = h.ws.dispatch(Action::AddComment).unwrap();
    assert!(
        matches!(effects.as_slice(), [Effect::Status(..)]),
        "{effects:?}"
    );
}

#[test]
fn spaceはカーソル行を覆うスレッドを開閉する() {
    let comments = vec![crate::review::tests::comment("a", "a.txt", 2, Some(4))];
    let dir = fixture(&[("a.txt", "1\n2\n3\n4\n5\n")]);
    let mut h = Harness::at(dir.path());
    h.open("a.txt");
    h.install(comments.clone());
    let all: Vec<&conductor_core::review_store::ReviewComment> = comments.iter().collect();

    h.viewer().scroll.line = 2;
    assert!(
        h.ws.panels.viewer.threads.is_open(&all, 4),
        "未解決は既定で開く"
    );
    h.ws.dispatch(Action::ToggleInlineThread).unwrap();
    assert!(
        !h.ws.panels.viewer.threads.is_open(&all, 4),
        "範囲の途中から終端のスレッドを閉じる"
    );
}

#[test]
fn 返信と解決はカーソル行のコメントに効きコメントが無ければ知らせる() {
    let comments = vec![crate::review::tests::comment("a", "a.txt", 2, None)];
    let dir = fixture(&[("a.txt", "1\n2\n3\n")]);
    let mut h = Harness::at(dir.path());
    h.open("a.txt");
    h.install(comments);

    h.viewer().scroll.line = 1;
    let effects = h.ws.dispatch(Action::ReplyToComment).unwrap();
    assert!(matches!(
        effects.as_slice(),
        [Effect::PushModal(Modal::CommentEditor(_))]
    ));
    let effects = h.ws.dispatch(Action::ToggleResolve).unwrap();
    assert!(matches!(effects.as_slice(), [Effect::Spawn(_)]));

    h.viewer().scroll.line = 2;
    for action in [Action::ReplyToComment, Action::ToggleResolve] {
        let effects = h.ws.dispatch(action).unwrap();
        assert!(
            matches!(effects.as_slice(), [Effect::Status(..)]),
            "{action:?}"
        );
    }
}

#[test]
fn コメント間の移動は今の行を飛ばして両端で止まる() {
    let comments = vec![
        crate::review::tests::comment("a", "a.txt", 2, None),
        crate::review::tests::comment("b", "a.txt", 5, None),
    ];
    let dir = fixture(&[("a.txt", "1\n2\n3\n4\n5\n6\n")]);
    let mut h = Harness::at(dir.path());
    h.open("a.txt");
    h.install(comments);

    let step = |h: &mut Harness, action| {
        let effects = h.ws.dispatch(action).unwrap();
        (h.ws.panels.viewer.scroll.line, effects)
    };
    assert_eq!(step(&mut h, Action::NextComment).0, 1);
    assert_eq!(step(&mut h, Action::NextComment).0, 4);
    let (line, effects) = step(&mut h, Action::NextComment);
    assert_eq!(line, 4, "末尾では動かない");
    assert!(matches!(effects.as_slice(), [Effect::Status(..)]));
    assert_eq!(step(&mut h, Action::PrevComment).0, 1);
}

const NOTES: &str = "# Title\n\nsome **bold** prose.\n\n| a | b |\n|---|---|\n| 1 | 2 |\n";

/// レンダリング表示に着地してから、行を指す操作 (ここではコードジャンプ) で
/// 素のソースへ戻るまで。行番号の無い表示に行を要求されたら抜けるしかない。
#[test]
fn mでレンダリング表示に入り行を指す操作で素のソースへ戻る() {
    let dir = fixture(&[("notes.md", NOTES)]);
    let mut h = Harness::at(dir.path());
    h.viewer().body = ratatui::layout::Rect::new(0, 1, 60, 20);
    h.open("notes.md");
    h.prepare();
    assert!(h.body()[0].starts_with("# Title"), "まずは素のソース");

    h.act(Action::ToggleMarkdownRender);
    h.prepare();
    assert!(h.ws.panels.viewer.is_showing_rendered_markdown());
    let rendered = h.rendered();
    assert!(
        rendered.iter().any(|l| l.contains("\u{2503} Title")),
        "見出しは色の帯付きで描く: {rendered:?}"
    );
    assert!(
        !rendered.iter().any(|l| l.contains("**bold**")),
        "記法は消えている: {rendered:?}"
    );

    h.act(Action::NavigateDown);
    assert_eq!(h.ws.panels.viewer.scroll.md, 1, "j は文書を送る");

    h.run(vec![Effect::JumpTo {
        path: PathBuf::from("notes.md"),
        line: 3,
    }]);
    assert!(!h.ws.panels.viewer.is_showing_rendered_markdown());
    assert_eq!(h.ws.panels.viewer.scroll.line, 2, "要求された行に着く");
}

/// 切り替えは常に文書の先頭に着地し、行に紐づいたままの操作を畳む。残すと
/// 画面に無い行の選択が生き続ける。
#[test]
fn 切り替えはスクロールを戻し行の選択を畳む() {
    let dir = fixture(&[("notes.md", NOTES)]);
    let mut h = Harness::at(dir.path());
    h.open("notes.md");
    h.prepare();
    h.viewer().selection.click(2, false);

    h.act(Action::ToggleMarkdownRender);
    assert_eq!(h.ws.panels.viewer.scroll.md, 0);
    assert!(h.ws.panels.viewer.selection.is_empty());

    h.prepare();
    h.act(Action::GoToBottom);
    assert!(h.ws.panels.viewer.scroll.md > 0);
    h.act(Action::ToggleMarkdownRender);
    assert_eq!(h.ws.panels.viewer.scroll.md, 0, "戻るときも先頭から");
}

/// unified diff は本文を組み直すと +/- の構造が壊れ、markdown でないファイルには
/// そもそも意味が無い。描画のトグルとキーの両方がこの 1 つの述語を読む。
#[test]
fn レンダリング表示は素のmarkdownファイルに限る() {
    let dir = fixture(&[("notes.md", NOTES), ("a.txt", "plain\n")]);
    let mut h = Harness::at(dir.path());

    h.open("a.txt");
    assert!(!h.ws.panels.viewer.markdown_toggle_available());
    let effects = h.viewer().toggle_markdown();
    assert!(matches!(effects.as_slice(), [Effect::Status(..)]));
    assert!(!h.ws.panels.viewer.is_showing_rendered_markdown());

    h.open("notes.md");
    h.act(Action::ToggleMarkdownRender);
    assert!(h.ws.panels.viewer.is_showing_rendered_markdown());

    let file_diff = FileDiff {
        path: "notes.md".into(),
        added_lines: 1,
        deleted_lines: 0,
        hunks: vec![conductor_core::diff_state::DiffHunk {
            lines: vec![conductor_core::diff_state::DiffLine {
                tag: conductor_core::diff_state::DiffLineTag::Insert,
                old_line_no: None,
                new_line_no: Some(1),
                inline_segments: Vec::new(),
                content: "# Title".into(),
            }],
            func_header: None,
        }],
    };
    let effects = h.viewer().open(
        Path::new("notes.md"),
        None,
        Some(Box::new(OpenDiff {
            source: DiffSource::working_tree("main"),
            file: file_diff,
        })),
        false,
    );
    h.run(effects);
    assert!(
        !h.ws.panels.viewer.is_showing_rendered_markdown(),
        "差分では畳む"
    );
}

/// 押した行のテストだけを走らせる。コマンドはシェルへ送るので、Viewer は
/// PTY を知らないまま済む。
#[test]
fn 実行ボタンを押すとテストのコマンドがシェルへ行く() {
    let dir = fixture(&[]);
    std::fs::create_dir(dir.path().join("src")).unwrap();
    std::fs::write(
        dir.path().join("src/lib.rs"),
        "#[cfg(test)]\nmod tests {\n    #[test]\n    fn works() {}\n}\n",
    )
    .unwrap();
    let mut h = Harness::at(dir.path());
    h.open("src/lib.rs");
    h.viewer().body = ratatui::layout::Rect::new(0, 10, 60, 20);

    // 桁は 行番号(1) + 折りたたみ(1) の右。4 行目が fn works。
    let effects = h.click(2, 13, false);
    let [Effect::Status(_, text), Effect::SendToShell(command)] = effects.as_slice() else {
        panic!("{effects:?}");
    };
    assert_eq!(text, "Running works");
    assert!(command.starts_with("cargo test"), "{command}");
    assert!(command.contains("works"), "{command}");

    assert!(
        h.click(20, 13, false).is_empty(),
        "本文の桁はテストを走らせない"
    );
}

/// デコードは svc のワーカー。鍵は (パス, 桁, 行) なので、区画が同じ間は
/// 描き直しを頼まない。
#[test]
fn 画像は区画の大きさごとに一度だけ描く() {
    let dir = fixture(&[]);
    let image = image::RgbImage::from_fn(8, 8, |x, _| image::Rgb([(x * 32) as u8, 0, 0]));
    image.save(dir.path().join("logo.png")).unwrap();
    let mut h = Harness::at(dir.path());

    h.open("logo.png");
    h.viewer().body = ratatui::layout::Rect::new(0, 1, 40, 12);
    assert!(
        matches!(
            h.ws.panels.viewer.content.media,
            Some(media::Preview::Loading)
        ),
        "描き上がるまでは Loading"
    );

    h.prepare();
    let Some(media::Preview::Ready(rendered)) = &h.ws.panels.viewer.content.media else {
        panic!("{:?}", h.ws.panels.viewer.content.media);
    };
    assert_eq!(rendered.dimensions, (8, 8));
    assert!(!rendered.lines.is_empty());

    let effects = {
        let Workspace {
            panels,
            config,
            theme,
            ..
        } = &mut h.ws;
        panels.viewer.prepare(config, theme)
    };
    assert!(effects.is_empty(), "同じ大きさなら描き直さない");
}

#[test]
fn 選択は起点から伸び上向きでも正規化される() {
    /// クリック列 (行, shift), 期待する範囲。
    type Case = (&'static [(usize, bool)], Option<(usize, usize)>);
    let cases: [Case; 4] = [
        (&[(7, false)], Some((7, 7))),
        (&[(5, false), (9, true)], Some((5, 9))),
        (&[(9, false), (4, true)], Some((4, 9))),
        (&[(3, true)], Some((3, 3))),
    ];
    for (clicks, expected) in cases {
        let mut selection = Selection::default();
        for (line, extend) in clicks {
            selection.click(*line, *extend);
        }
        assert_eq!(selection.range(), expected, "{clicks:?}");
    }
}

#[test]
fn 選択の判定は両端を含む() {
    let mut selection = Selection::default();
    selection.click(3, false);
    selection.click(5, true);
    assert!(!selection.contains(2));
    assert!(selection.contains(3) && selection.contains(5));
    assert!(!selection.contains(6));
    selection.clear();
    assert!(selection.is_empty() && !selection.contains(4));
}
