//! 既知リポジトリの切替。開く方はパス入力の [crate::modal::Prompt] が兼ねる。

use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::effect::Effect;
use crate::modal::picker::Cursor;
use crate::workspace::Ctx;

#[derive(Debug)]
pub struct RepoPicker {
    pub cursor: Cursor,
}

impl RepoPicker {
    pub fn open(current: usize) -> Self {
        Self {
            cursor: Cursor { selected: current },
        }
    }

    pub fn update(&mut self, key: KeyEvent, ctx: &Ctx) -> Vec<Effect> {
        let known = &ctx.repo.known;
        if self.cursor.navigate(ctx.keymap, key, known.len()) {
            return Vec::new();
        }
        match key.code {
            KeyCode::Enter => match known.get(self.cursor.selected) {
                Some(path) => vec![Effect::PopModal, Effect::SwitchRepo(path.clone())],
                None => vec![Effect::PopModal],
            },
            KeyCode::Esc => vec![Effect::PopModal],
            _ => Vec::new(),
        }
    }
}

/// パス入力で開くときの入口。`~` はホームに展開する。
pub fn expand_home(input: &str) -> PathBuf {
    let Some(rest) = input.strip_prefix('~') else {
        return PathBuf::from(input);
    };
    match dirs::home_dir() {
        Some(home) => home.join(rest.strip_prefix('/').unwrap_or(rest)),
        None => PathBuf::from(input),
    }
}

/// シェルの Tab と同じく、入力の最後の `/` より後ろをフォルダ名の頭として補う。
/// 候補が 1 つならその名前と `/` まで、複数なら共通する所まで伸ばし、候補も返す。
/// 入力の `~` は読むときだけ展開し、綴りはそのまま残す。
pub fn complete_dir(input: &str) -> (String, Vec<String>) {
    let (head, stem) = input.split_at(input.rfind('/').map_or(0, |i| i + 1));
    let dir = match head {
        "" => PathBuf::from("."),
        head => expand_home(head),
    };
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| Some((entry.file_name().into_string().ok()?, entry.path())))
        .filter(|(name, _)| {
            name.starts_with(stem) && (stem.starts_with('.') || !name.starts_with('.'))
        })
        .filter(|(_, path)| path.is_dir())
        .map(|(name, _)| name)
        .collect();
    names.sort();
    match names.as_slice() {
        [] => (input.to_string(), Vec::new()),
        [only] => (format!("{head}{only}/"), Vec::new()),
        [first, rest @ ..] => {
            let common = rest
                .iter()
                .fold(first.as_str(), |common, name| common_prefix(common, name));
            (format!("{head}{common}"), names.clone())
        }
    }
}

fn common_prefix<'a>(a: &'a str, b: &str) -> &'a str {
    let len = a
        .char_indices()
        .zip(b.chars())
        .take_while(|((_, x), y)| x == y)
        .last()
        .map_or(0, |((i, c), _)| i + c.len_utf8());
    &a[..len]
}

pub fn title() -> String {
    "Switch Repository".into()
}

pub fn lines(picker: &RepoPicker, ctx: &Ctx) -> Vec<Line<'static>> {
    ctx.repo
        .known
        .iter()
        .enumerate()
        .map(|(i, path)| {
            let marker = if *path == ctx.repo.root { "* " } else { "  " };
            crate::list::row_line(
                vec![
                    Span::styled(marker, Style::default().fg(ctx.theme.accent)),
                    Span::styled(
                        path.display().to_string(),
                        Style::default().fg(ctx.theme.fg),
                    ),
                ],
                ctx.theme,
                i == picker.cursor.selected,
                true,
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn チルダはホームに展開する() {
        let home = dirs::home_dir().unwrap();
        assert_eq!(expand_home("~/src"), home.join("src"));
        assert_eq!(expand_home("~"), home);
        assert_eq!(expand_home("/abs/path"), PathBuf::from("/abs/path"));
    }

    fn tree(dirs: &[&str], files: &[&str]) -> (tempfile::TempDir, String) {
        let tmp = tempfile::tempdir().unwrap();
        for dir in dirs {
            std::fs::create_dir(tmp.path().join(dir)).unwrap();
        }
        for file in files {
            std::fs::write(tmp.path().join(file), "").unwrap();
        }
        let base = format!("{}/", tmp.path().display());
        (tmp, base)
    }

    #[test]
    fn 候補が一つならフォルダ名とスラッシュまで埋める() {
        let (_tmp, base) = tree(&["conductor", "sheaf"], &["config.toml"]);
        assert_eq!(
            complete_dir(&format!("{base}con")),
            (format!("{base}conductor/"), Vec::new())
        );
    }

    #[test]
    fn 候補が複数なら共通する所まで伸ばして候補を返す() {
        let (_tmp, base) = tree(&["conductor-worktrees", "conductor", "sheaf"], &[]);
        assert_eq!(
            complete_dir(&format!("{base}co")),
            (
                format!("{base}conductor"),
                vec!["conductor".to_string(), "conductor-worktrees".to_string()]
            )
        );
    }

    #[test]
    fn スラッシュで終わればその中のフォルダが全部候補になり_隠しフォルダは除く() {
        let (_tmp, base) = tree(&["b", "a", ".git"], &["file"]);
        assert_eq!(
            complete_dir(&base),
            (base.clone(), vec!["a".to_string(), "b".to_string()])
        );
        assert_eq!(
            complete_dir(&format!("{base}.")),
            (format!("{base}.git/"), Vec::new())
        );
    }

    #[test]
    fn フォルダへのリンクは候補になる() {
        let (tmp, base) = tree(&["real"], &[]);
        std::os::unix::fs::symlink(tmp.path().join("real"), tmp.path().join("link")).unwrap();
        assert_eq!(complete_dir(&format!("{base}l")).0, format!("{base}link/"));
    }

    #[test]
    fn 候補が無いか読めないフォルダなら入力はそのまま() {
        let (_tmp, base) = tree(&["a"], &[]);
        for input in [format!("{base}zz"), format!("{base}missing/x")] {
            assert_eq!(complete_dir(&input), (input.clone(), Vec::new()));
        }
    }

    #[test]
    fn チルダの綴りを残したまま補う() {
        let home = dirs::home_dir().unwrap();
        let only = std::fs::read_dir(&home)
            .unwrap()
            .flatten()
            .filter(|e| e.path().is_dir())
            .filter_map(|e| e.file_name().into_string().ok())
            .find(|name| !name.starts_with('.'))
            .unwrap();
        let (text, _) = complete_dir(&format!("~/{only}"));
        assert!(text.starts_with(&format!("~/{only}")), "{text}");
    }
}
