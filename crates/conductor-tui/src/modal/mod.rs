//! 入力を独占するもの。1 バリアント 1 ファイルで状態・update・render を同居させる。
//! スタックの top だけが入力を受け、既定は消費する (IME の変換中グリフを外に漏らさない)。

use conductor_core::keymap::KeyContext;
use conductor_core::review_store::{CommentKind, ReviewComment, ReviewReply};
use conductor_core::text_input::TextInput;
use conductor_core::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::comment_list::CommentList;
use crate::effect::Effect;
use crate::fx::{Kind, Target};
use crate::task::{ReviewWrite, Task};
use crate::workspace::{Ctx, StatusLevel};

pub mod branch;
pub mod commits;
pub mod confirm;
pub mod grep;
pub mod help;
pub mod history;
pub mod input;
pub mod palette;
pub mod picker;
pub mod popup;
pub mod pr;
pub mod publish;
pub mod references;
pub mod repo;
pub mod session;
pub mod theme;
pub mod update;

pub use confirm::Confirm;

#[derive(Debug)]
pub enum Modal {
    Help(help::Help),
    Prompt(Prompt),
    Confirm(Confirm),
    CommentEditor(CommentEditor),
    CommentList(CommentList),
    Palette(palette::Palette),
    ThemePicker(theme::ThemePicker),
    RepoPicker(repo::RepoPicker),
    Resume(session::ResumePicker),
    History(history::HistoryBrowser),
    Grep(grep::Grep),
    BranchPicker(branch::BranchPicker),
    CherryPick(commits::CherryPick),
    PrInput(pr::PrInput),
    Publish(publish::Publish),
    Update(update::Update),
    References(references::References),
    Popup(popup::Popup),
}

/// 1 行のテキスト入力。確定した文字列は選択中の [Mode] が Effect に変える。
#[derive(Debug)]
pub struct Prompt {
    pub title: String,
    pub input: TextInput,
    /// Tab で巡る宛先。打ちかけの本文は持ち越す。
    modes: Vec<Mode>,
    selected: usize,
    /// Tab でフォルダ名を補う入力欄だけ Some。中身は前の Tab で絞りきれなかった候補。
    dir_candidates: Option<Vec<String>>,
}

#[derive(Debug)]
pub struct Mode {
    pub label: String,
    pub on_submit: fn(String) -> Vec<Effect>,
}

impl Prompt {
    pub fn single(title: &str, on_submit: fn(String) -> Vec<Effect>) -> Self {
        Self::with_modes(
            title,
            vec![Mode {
                label: String::new(),
                on_submit,
            }],
        )
    }

    pub fn with_modes(title: &str, modes: Vec<Mode>) -> Self {
        Self {
            title: title.to_string(),
            input: TextInput::new(),
            modes,
            selected: 0,
            dir_candidates: None,
        }
    }

    pub fn completing_dirs(mut self) -> Self {
        self.dir_candidates = Some(Vec::new());
        self
    }

    fn complete_dir(&mut self) {
        let (text, candidates) = repo::complete_dir(self.input.text());
        self.input.set_text(&text);
        self.dir_candidates = Some(candidates);
    }

    fn forget_candidates(&mut self) {
        if let Some(candidates) = &mut self.dir_candidates {
            candidates.clear();
        }
    }

    pub fn mode(&self) -> &Mode {
        &self.modes[self.selected]
    }

    fn has_modes(&self) -> bool {
        self.modes.len() > 1
    }

    fn next_mode(&mut self) {
        self.selected = (self.selected + 1) % self.modes.len();
    }
}

pub const PROMPT_MARK: &str = "> ";

/// 宛先の切替タブと入力欄。
pub fn prompt_lines(prompt: &Prompt, theme: &Theme, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if prompt.has_modes() {
        let mut spans = Vec::new();
        for (i, mode) in prompt.modes.iter().enumerate() {
            let style = if i == prompt.selected {
                Style::default()
                    .fg(theme.selected_fg)
                    .bg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.hint)
            };
            spans.push(Span::styled(format!(" {} ", mode.label), style));
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(
            " Tab: switch",
            Style::default().fg(theme.hint),
        ));
        lines.push(Line::from(spans));
        lines.push(Line::from(""));
    }
    lines.extend(
        input::with_caret(&prompt.input, width.saturating_sub(PROMPT_MARK.len()))
            .into_iter()
            .map(|line| {
                Line::styled(
                    format!("{PROMPT_MARK}{line}"),
                    Style::default().fg(theme.fg),
                )
            }),
    );
    let candidates = prompt.dir_candidates.as_deref().unwrap_or_default();
    if !candidates.is_empty() {
        lines.push(Line::from(""));
        lines.extend(
            candidate_rows(candidates, width)
                .into_iter()
                .map(|row| Line::styled(row, Style::default().fg(theme.hint))),
        );
    }
    lines
}

const CANDIDATE_ROWS: usize = 6;

/// 候補を横に詰めて並べ、入りきらない分は数だけ出す。
fn candidate_rows(candidates: &[String], width: usize) -> Vec<String> {
    use unicode_width::UnicodeWidthStr;
    let mut rows: Vec<String> = vec![String::new()];
    for (shown, name) in candidates.iter().enumerate() {
        let cell = format!("{name}/");
        let row = rows.last_mut().expect("空にしない");
        if row.is_empty() || row.width() + 2 + cell.width() <= width {
            if !row.is_empty() {
                row.push_str("  ");
            }
            row.push_str(&cell);
            continue;
        }
        if rows.len() == CANDIDATE_ROWS {
            rows.push(format!("… and {} more", candidates.len() - shown));
            return rows;
        }
        rows.push(cell);
    }
    rows
}

/// 1 つの入力欄が兼ねる 4 つの書き込み先。
#[derive(Debug)]
pub enum EditTarget {
    New {
        file_path: String,
        line_start: u32,
        line_end: Option<u32>,
    },
    Comment {
        id: String,
    },
    NewReply {
        comment_id: String,
    },
    Reply {
        id: String,
    },
}

/// コメント本文の複数行入力。
#[derive(Debug)]
pub struct CommentEditor {
    pub target: EditTarget,
    /// 新規のときだけ tab で入れ替わる。
    pub kind: CommentKind,
    pub input: TextInput,
}

impl CommentEditor {
    pub fn new_comment(file_path: String, line_start: u32, line_end: Option<u32>) -> Self {
        Self {
            target: EditTarget::New {
                file_path,
                line_start,
                line_end,
            },
            kind: CommentKind::Suggest,
            input: TextInput::new_multiline(),
        }
    }

    pub fn edit_comment(comment: &ReviewComment) -> Self {
        let mut input = TextInput::new_multiline();
        input.set_text(&comment.body);
        Self {
            target: EditTarget::Comment {
                id: comment.id.clone(),
            },
            kind: comment.kind,
            input,
        }
    }

    pub fn reply_to(comment: &ReviewComment) -> Self {
        Self {
            target: EditTarget::NewReply {
                comment_id: comment.id.clone(),
            },
            kind: comment.kind,
            input: TextInput::new_multiline(),
        }
    }

    pub fn edit_reply(reply: &ReviewReply) -> Self {
        let mut input = TextInput::new_multiline();
        input.set_text(&reply.body);
        Self {
            target: EditTarget::Reply {
                id: reply.id.clone(),
            },
            kind: CommentKind::Suggest,
            input,
        }
    }

    pub fn title(&self) -> String {
        match &self.target {
            EditTarget::New { .. } => format!("New {} (tab: kind)", self.kind),
            EditTarget::Comment { .. } => "Edit comment".into(),
            EditTarget::NewReply { .. } => "Reply".into(),
            EditTarget::Reply { .. } => "Edit reply".into(),
        }
    }

    fn submit(&self) -> Vec<Effect> {
        let body = self.input.text().trim().to_string();
        if body.is_empty() {
            return vec![Effect::Status(
                StatusLevel::Warning,
                "the comment body is empty".into(),
            )];
        }
        let write = match &self.target {
            EditTarget::New {
                file_path,
                line_start,
                line_end,
            } => ReviewWrite::AddComment {
                file_path: file_path.clone(),
                line_start: *line_start,
                line_end: *line_end,
                kind: self.kind,
                body,
            },
            EditTarget::Comment { id } => ReviewWrite::EditComment {
                id: id.clone(),
                body,
            },
            EditTarget::NewReply { comment_id } => ReviewWrite::AddReply {
                comment_id: comment_id.clone(),
                body,
            },
            EditTarget::Reply { id } => ReviewWrite::EditReply {
                id: id.clone(),
                body,
            },
        };
        vec![Effect::PopModal, Effect::Spawn(Task::WriteReview(write))]
    }

    fn key(&mut self, key: KeyEvent) -> Vec<Effect> {
        match key.code {
            KeyCode::Esc => vec![Effect::PopModal],
            // 改行は shift+enter。enter を改行にすると確定の手段が無くなる。
            KeyCode::Enter if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.input.insert_char('\n');
                Vec::new()
            }
            KeyCode::Enter => self.submit(),
            KeyCode::Tab if matches!(self.target, EditTarget::New { .. }) => {
                self.kind = match self.kind {
                    CommentKind::Suggest => CommentKind::Question,
                    CommentKind::Question => CommentKind::Suggest,
                };
                Vec::new()
            }
            _ => {
                self.input.handle_key(key);
                Vec::new()
            }
        }
    }
}

impl Modal {
    pub fn tick(&mut self, ctx: &Ctx) -> Vec<Effect> {
        match self {
            Modal::Grep(grep) => grep.tick(ctx.root),
            _ => Vec::new(),
        }
    }

    /// 貼り付けを入力欄へ入れる。改行を落とすかは TextInput が単一行かで決まる。
    /// ワイルドカードで受けないのは、入力欄を持つバリアントが増えたときに
    /// 黙って貼り付けを捨てるのを防ぐため。
    pub fn paste(&mut self, text: &str) {
        match self {
            Modal::Prompt(prompt) => {
                prompt.forget_candidates();
                prompt.input.insert_str(text);
            }
            Modal::CommentEditor(editor) => editor.input.insert_str(text),
            Modal::PrInput(prompt) => prompt.paste(text),
            Modal::Grep(grep) => grep.paste(text),
            Modal::History(browser) => browser.paste(text),
            Modal::Palette(palette) => {
                picker::filtered_paste(&mut palette.cursor, &mut palette.input, text)
            }
            Modal::BranchPicker(picker) => {
                picker::filtered_paste(&mut picker.cursor, &mut picker.filter, text)
            }
            Modal::Resume(picker) => {
                picker::filtered_paste(&mut picker.cursor, &mut picker.filter, text)
            }
            Modal::Help(_)
            | Modal::Confirm(_)
            | Modal::CommentList(_)
            | Modal::ThemePicker(_)
            | Modal::RepoPicker(_)
            | Modal::CherryPick(_)
            | Modal::Publish(_)
            | Modal::Update(_)
            | Modal::References(_)
            | Modal::Popup(_) => {}
        }
    }

    pub fn update(&mut self, key: KeyEvent, ctx: &Ctx) -> Vec<Effect> {
        match self {
            Modal::Help(help) => help.update(key),
            Modal::Palette(palette) => palette.update(key, ctx),
            Modal::ThemePicker(picker) => picker.update(key, ctx),
            Modal::RepoPicker(picker) => picker.update(key, ctx),
            Modal::Resume(picker) => picker.update(key, ctx),
            Modal::History(browser) => browser.update(key, ctx),
            Modal::Grep(grep) => grep.update(key, ctx),
            Modal::BranchPicker(picker) => picker.update(key, ctx),
            Modal::CherryPick(picker) => picker.update(key, ctx),
            Modal::PrInput(prompt) => prompt.update(key, ctx),
            Modal::Publish(confirm) => confirm.update(key, ctx),
            Modal::Update(update) => update.update(key, ctx),
            Modal::References(list) => list.update(key, ctx),
            Modal::Popup(popup) => popup.update(key, ctx),
            Modal::Prompt(prompt) => match key.code {
                KeyCode::Esc => vec![Effect::PopModal],
                KeyCode::Tab if prompt.dir_candidates.is_some() => {
                    prompt.complete_dir();
                    vec![]
                }
                KeyCode::Tab if prompt.has_modes() => {
                    prompt.next_mode();
                    vec![Effect::Play(Kind::Flash, Target::Modal)]
                }
                KeyCode::Enter => {
                    let mut effects = vec![Effect::PopModal];
                    effects.extend((prompt.mode().on_submit)(prompt.input.text().to_string()));
                    effects
                }
                _ => {
                    prompt.forget_candidates();
                    prompt.input.handle_key(key);
                    vec![]
                }
            },
            Modal::Confirm(confirm) => confirm.update(key),
            Modal::CommentEditor(editor) => editor.key(key),
            Modal::CommentList(list) => {
                if key.code == KeyCode::Esc {
                    return vec![Effect::PopModal];
                }
                let action = ctx.keymap.resolve(&key, KeyContext::ExplorerCommentList);
                let effects = action
                    .and_then(|action| list.update(action, ctx.review))
                    .unwrap_or_default();
                crate::comment_list::jump_effects(effects)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::tests::comment as fixture;
    use crate::workspace::Workspace;

    fn press(editor: &mut CommentEditor, keys: &[KeyEvent]) -> Vec<Effect> {
        let mut last = Vec::new();
        for key in keys {
            last = editor.key(*key);
        }
        last
    }

    fn ch(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    #[test]
    fn 空の本文は保存せず理由を出す() {
        let mut editor = CommentEditor::new_comment("a.rs".into(), 1, None);
        let effects = press(&mut editor, &[KeyEvent::from(KeyCode::Enter)]);
        assert!(
            matches!(effects.as_slice(), [Effect::Status(..)]),
            "{effects:?}"
        );

        let effects = press(&mut editor, &[ch(' '), KeyEvent::from(KeyCode::Enter)]);
        assert!(
            matches!(effects.as_slice(), [Effect::Status(..)]),
            "空白だけ"
        );
    }

    #[test]
    fn 改行はshift_enterで本文に入る() {
        let mut editor = CommentEditor::new_comment("a.rs".into(), 2, Some(4));
        press(
            &mut editor,
            &[
                ch('a'),
                KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT),
                ch('b'),
            ],
        );
        assert_eq!(editor.input.text(), "a\nb");

        let effects = press(&mut editor, &[KeyEvent::from(KeyCode::Enter)]);
        let [
            Effect::PopModal,
            Effect::Spawn(Task::WriteReview(ReviewWrite::AddComment {
                line_start,
                line_end,
                body,
                ..
            })),
        ] = effects.as_slice()
        else {
            panic!("{effects:?}");
        };
        assert_eq!(
            (*line_start, *line_end, body.as_str()),
            (2, Some(4), "a\nb")
        );
    }

    #[test]
    fn tabは新規のときだけ種別を入れ替える() {
        let mut new = CommentEditor::new_comment("a.rs".into(), 1, None);
        press(&mut new, &[KeyEvent::from(KeyCode::Tab)]);
        assert_eq!(new.kind, CommentKind::Question);
        assert!(new.title().contains("question"), "{}", new.title());

        let mut edit = CommentEditor::edit_comment(&fixture("a", "a.rs", 1, None));
        let before = edit.kind;
        press(&mut edit, &[KeyEvent::from(KeyCode::Tab)]);
        assert_eq!(edit.kind, before);
    }

    #[test]
    fn 編集は今の本文から始まり同じidへ書き戻す() {
        let mut editor = CommentEditor::edit_comment(&fixture("cid", "a.rs", 1, None));
        assert_eq!(editor.input.text(), "body of cid");
        let effects = press(&mut editor, &[ch('!'), KeyEvent::from(KeyCode::Enter)]);
        let [
            _,
            Effect::Spawn(Task::WriteReview(ReviewWrite::EditComment { id, body })),
        ] = effects.as_slice()
        else {
            panic!("{effects:?}");
        };
        assert_eq!((id.as_str(), body.as_str()), ("cid", "body of cid!"));
    }

    #[test]
    fn escは何も書かずに閉じる() {
        let mut editor = CommentEditor::new_comment("a.rs".into(), 1, None);
        let effects = press(&mut editor, &[ch('x'), KeyEvent::from(KeyCode::Esc)]);
        assert_eq!(effects.len(), 1);
        assert!(matches!(effects[0], Effect::PopModal));
    }

    #[test]
    fn 貼り付けは単一行では改行を落とし複数行では残す() {
        let mut prompt = Modal::Prompt(Prompt::single("t", |_| Vec::new()));
        prompt.paste("ab\ncd");
        let Modal::Prompt(prompt) = &prompt else {
            unreachable!()
        };
        assert_eq!(prompt.input.text(), "abcd");

        let mut editor = Modal::CommentEditor(CommentEditor::new_comment("a.rs".into(), 1, None));
        editor.paste("ab\ncd");
        let Modal::CommentEditor(editor) = &editor else {
            unreachable!()
        };
        assert_eq!(editor.input.text(), "ab\ncd");
    }

    #[test]
    fn tabは宛先を巡り打ちかけの本文を残す() {
        let ws = Workspace::for_test();
        let mut modal = Modal::Prompt(Prompt::with_modes(
            "new",
            vec![
                Mode {
                    label: "branch".into(),
                    on_submit: |_| vec![Effect::PopModal],
                },
                Mode {
                    label: "task".into(),
                    on_submit: |_| vec![Effect::Quit],
                },
            ],
        ));
        modal.paste("wip");
        let effects = modal.update(KeyEvent::from(KeyCode::Tab), &ws.ctx());
        assert!(
            matches!(
                effects.as_slice(),
                [Effect::Play(Kind::Flash, Target::Modal)]
            ),
            "切替の合図が出ない: {effects:?}"
        );

        let Modal::Prompt(prompt) = &modal else {
            unreachable!()
        };
        assert_eq!(prompt.mode().label, "task");
        assert_eq!(prompt.input.text(), "wip");
        assert!(matches!(
            (prompt.mode().on_submit)("wip".into()).as_slice(),
            [Effect::Quit]
        ));

        modal.update(KeyEvent::from(KeyCode::Tab), &ws.ctx());
        let Modal::Prompt(prompt) = &modal else {
            unreachable!()
        };
        assert_eq!(prompt.mode().label, "branch");
        assert_eq!(prompt.input.text(), "wip");
    }

    #[test]
    fn 宛先が一つならtabは何もしない() {
        let ws = Workspace::for_test();
        let mut modal = Modal::Prompt(Prompt::single("t", |_| Vec::new()));
        let effects = modal.update(KeyEvent::from(KeyCode::Tab), &ws.ctx());
        assert!(effects.is_empty(), "{effects:?}");
    }

    fn dir_prompt(dirs: &[&str], typed: &str) -> (tempfile::TempDir, Modal) {
        let tmp = tempfile::tempdir().unwrap();
        for dir in dirs {
            std::fs::create_dir(tmp.path().join(dir)).unwrap();
        }
        let mut prompt = Prompt::single("t", |_| Vec::new()).completing_dirs();
        prompt
            .input
            .set_text(&format!("{}/{typed}", tmp.path().display()));
        (tmp, Modal::Prompt(prompt))
    }

    fn prompt_of(modal: &Modal) -> &Prompt {
        let Modal::Prompt(prompt) = modal else {
            unreachable!()
        };
        prompt
    }

    #[test]
    fn フォルダ補完の入力欄ではtabで名前が埋まる() {
        let ws = Workspace::for_test();
        let (tmp, mut modal) = dir_prompt(&["conductor"], "con");
        modal.update(KeyEvent::from(KeyCode::Tab), &ws.ctx());
        assert_eq!(
            prompt_of(&modal).input.text(),
            format!("{}/conductor/", tmp.path().display())
        );
    }

    #[test]
    fn 絞りきれない候補は入力欄の下に出て_打つと消える() {
        let ws = Workspace::for_test();
        let theme = Theme::from_name("dracula");
        let (_tmp, mut modal) = dir_prompt(&["alpha", "beta"], "");
        let shown = |modal: &Modal| {
            prompt_lines(prompt_of(modal), &theme, 200)
                .iter()
                .any(|line| line.to_string() == "alpha/  beta/")
        };

        modal.update(KeyEvent::from(KeyCode::Tab), &ws.ctx());
        assert!(shown(&modal));
        modal.update(KeyEvent::from(KeyCode::Char('a')), &ws.ctx());
        assert!(!shown(&modal));

        modal.update(KeyEvent::from(KeyCode::Backspace), &ws.ctx());
        modal.update(KeyEvent::from(KeyCode::Tab), &ws.ctx());
        modal.paste("x");
        assert!(!shown(&modal));
    }

    #[test]
    fn 候補は決めた行数で打ち切り_残りは数で出す() {
        let names: Vec<String> = (0..20).map(|i| format!("dir{i:02}")).collect();
        let rows = candidate_rows(&names, 6);
        assert_eq!(rows.len(), CANDIDATE_ROWS + 1);
        assert_eq!(rows.last().unwrap(), "… and 14 more");
    }

    /// 文言が変わるだけでは、どちらにいるか一目で分からない。
    #[test]
    fn 切替タブは選択中の宛先だけを塗る() {
        let theme = Theme::from_name("dracula");
        let mut prompt = Prompt::with_modes(
            "new",
            vec![
                Mode {
                    label: "branch".into(),
                    on_submit: |_| Vec::new(),
                },
                Mode {
                    label: "task".into(),
                    on_submit: |_| Vec::new(),
                },
            ],
        );
        let painted = |prompt: &Prompt| -> Vec<String> {
            prompt_lines(prompt, &theme, 40)[0]
                .spans
                .iter()
                .filter(|s| s.style.bg == Some(theme.accent))
                .map(|s| s.content.trim().to_string())
                .collect()
        };
        assert_eq!(painted(&prompt), ["branch"]);
        prompt.next_mode();
        assert_eq!(painted(&prompt), ["task"]);

        let single = Prompt::single("t", |_| Vec::new());
        assert!(
            prompt_lines(&single, &theme, 40)[0]
                .to_string()
                .starts_with("> ")
        );
    }

    #[test]
    fn 一覧モーダルからの移動は閉じてviewerへ渡す() {
        let mut ws = Workspace::for_test();
        ws.review.install(Ok(crate::review::Snapshot {
            branch: "main".into(),
            comments: vec![fixture("a", "a.rs", 7, None)],
            ..crate::review::Snapshot::default()
        }));
        let mut modal = Modal::CommentList(crate::comment_list::CommentList::default());
        let effects = modal.update(KeyEvent::from(KeyCode::Enter), &ws.ctx());
        assert!(
            matches!(
                effects.as_slice(),
                [
                    Effect::PopModal,
                    Effect::OpenFile { .. },
                    Effect::Focus(crate::workspace::Focus::Viewer)
                ]
            ),
            "{effects:?}"
        );

        let effects = modal.update(KeyEvent::from(KeyCode::Esc), &ws.ctx());
        assert!(matches!(effects.as_slice(), [Effect::PopModal]));
    }
}
