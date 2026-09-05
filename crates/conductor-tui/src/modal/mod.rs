//! 入力を独占するもの。1 バリアント 1 ファイルで状態・update・render を同居させる。
//! スタックの top だけが入力を受け、既定は消費する (IME の変換中グリフを外に漏らさない)。

use conductor_core::keymap::KeyContext;
use conductor_core::review_store::{CommentKind, ReviewComment, ReviewReply};
use conductor_core::text_input::TextInput;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::comment_list::CommentList;
use crate::effect::Effect;
use crate::task::{ReviewWrite, Task};
use crate::workspace::{Ctx, StatusLevel};

pub mod branch;
pub mod commits;
pub mod grep;
pub mod help;
pub mod history;
pub mod input;
pub mod palette;
pub mod picker;
pub mod pr;
pub mod publish;
pub mod references;
pub mod repo;
pub mod revidere;
pub mod session;
pub mod symbol_actions;
pub mod theme;
pub mod update;

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
    SymbolActions(symbol_actions::SymbolActions),
    RevidereConfirm(revidere::RevidereConfirm),
}

/// 1 行のテキスト入力。確定した文字列は `on_submit` が Effect に変える。
#[derive(Debug)]
pub struct Prompt {
    pub title: String,
    pub input: TextInput,
    pub on_submit: fn(String) -> Vec<Effect>,
    /// Tab で入れ替わるもう一つの宛先。打ちかけの本文は持ち越す。
    pub alternate: Option<Alternate>,
}

/// [Prompt] のもう一つの面。
#[derive(Debug)]
pub struct Alternate {
    pub title: String,
    pub on_submit: fn(String) -> Vec<Effect>,
}

impl Prompt {
    fn flip(&mut self) {
        let Some(alternate) = self.alternate.take() else {
            return;
        };
        self.alternate = Some(Alternate {
            title: std::mem::replace(&mut self.title, alternate.title),
            on_submit: std::mem::replace(&mut self.on_submit, alternate.on_submit),
        });
    }
}

/// y で発火する Effect を積んだ確認。開いた側が対象を捕まえたまま作れるよう、
/// 閉包ではなく組み立て済みの Effect を持つ。
#[derive(Debug)]
pub struct Confirm {
    pub question: String,
    pub on_yes: Vec<Effect>,
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

/// y / enter で積んだ Effect を流し、n / esc で閉じるだけの確認。
fn yes_no(key: KeyEvent, on_yes: &mut Vec<Effect>) -> Vec<Effect> {
    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
            let mut effects = vec![Effect::PopModal];
            effects.append(on_yes);
            effects
        }
        KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => vec![Effect::PopModal],
        _ => Vec::new(),
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
            Modal::Prompt(prompt) => prompt.input.insert_str(text),
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
            | Modal::SymbolActions(_)
            | Modal::RevidereConfirm(_) => {}
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
            Modal::SymbolActions(actions) => actions.update(key, ctx),
            Modal::Prompt(prompt) => match key.code {
                KeyCode::Esc => vec![Effect::PopModal],
                KeyCode::Tab if prompt.alternate.is_some() => {
                    prompt.flip();
                    vec![]
                }
                KeyCode::Enter => {
                    let mut effects = vec![Effect::PopModal];
                    effects.extend((prompt.on_submit)(prompt.input.text().to_string()));
                    effects
                }
                _ => {
                    prompt.input.handle_key(key);
                    vec![]
                }
            },
            Modal::Confirm(confirm) => yes_no(key, &mut confirm.on_yes),
            Modal::RevidereConfirm(confirm) => yes_no(key, &mut confirm.on_yes),
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
        let mut prompt = Modal::Prompt(Prompt {
            title: "t".into(),
            input: TextInput::new(),
            on_submit: |_| Vec::new(),
            alternate: None,
        });
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
    fn tabは宛先を入れ替え打ちかけの本文を残す() {
        let ws = Workspace::for_test();
        let mut modal = Modal::Prompt(Prompt {
            title: "branch".into(),
            input: TextInput::new(),
            on_submit: |_| vec![Effect::PopModal],
            alternate: Some(Alternate {
                title: "task".into(),
                on_submit: |_| vec![Effect::Quit],
            }),
        });
        modal.paste("wip");
        modal.update(KeyEvent::from(KeyCode::Tab), &ws.ctx());

        let Modal::Prompt(prompt) = &modal else {
            unreachable!()
        };
        assert_eq!(prompt.title, "task");
        assert_eq!(prompt.input.text(), "wip");
        assert!(matches!(
            (prompt.on_submit)("wip".into()).as_slice(),
            [Effect::Quit]
        ));

        modal.update(KeyEvent::from(KeyCode::Tab), &ws.ctx());
        let Modal::Prompt(prompt) = &modal else {
            unreachable!()
        };
        assert_eq!(prompt.title, "branch");
        assert_eq!(prompt.input.text(), "wip");
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
