//! ツアーの台本。`<worktree>/.conductor/tour.json` を読む。
//!
//! 台本はコードを持たない。どこを・どの順で・なんと言って見せるかだけを持ち、中身は
//! 画面が本物のファイルを開いて出す。台本が写しを持つと、コードが動いた瞬間に嘘になる。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::diff_state::{DiffLineTag, FileDiff};

/// 台本のスキーマ版。破壊的変更で上げる。
pub const SCHEMA_VERSION: u32 = 1;

/// 隣り合う行をひとまとめにする幅。これより離れていたら別の場所にする — 間の関係ない
/// 行まで連れてくると、何を見落としていたのかが埋もれる。
const GAP: u32 = 4;

pub fn artifact_path(worktree: &Path) -> PathBuf {
    worktree.join(".conductor").join("tour.json")
}

/// 行番号がどちら側のものか。
///
/// 削除行を後像の行番号に寄せる妥協をしないために一級で持つ。寄せると「消えた行の
/// 近く」を指すだけになり、指した先に本人が居ない。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    #[default]
    New,
    Old,
}

/// 停留所が指す場所。行番号は 1 始まり、両端を含む。
///
/// 範囲を省いた場所はファイルの頭を指す。囲んでいる関数へ広げるのは読む側の仕事で、
/// ここでは書かれた通りに持つ。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Place {
    pub path: String,
    #[serde(default)]
    pub side: Side,
    #[serde(default)]
    pub start: Option<u32>,
    #[serde(default)]
    pub end: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stop {
    pub title: String,
    #[serde(default)]
    pub narration: String,
    pub places: Vec<Place>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tour {
    pub schema_version: u32,
    #[serde(default)]
    pub title: String,
    pub stops: Vec<Stop>,
}

impl Place {
    pub fn whole(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            side: Side::New,
            start: None,
            end: None,
        }
    }

    /// 範囲を持たない場所はファイル全体なので、どちら側の行も説明したことになる。
    fn covers(&self, path: &str, side: Side, line: u32) -> bool {
        self.path == path
            && match (self.start, self.end) {
                (None, _) => true,
                _ if self.side != side => false,
                (Some(start), Some(end)) => (start..=end).contains(&line),
                (Some(start), None) => start == line,
            }
    }
}

impl Tour {
    pub fn from_json(text: &str) -> Result<Self, String> {
        let tour: Self = serde_json::from_str(text).map_err(|e| e.to_string())?;
        if tour.schema_version != SCHEMA_VERSION {
            return Err(format!(
                "スキーマ版 {} は読めない (このバイナリは {SCHEMA_VERSION})",
                tour.schema_version
            ));
        }
        Ok(tour)
    }

    fn places(&self) -> impl Iterator<Item = &Place> {
        self.stops.iter().flat_map(|stop| &stop.places)
    }
}

/// 台本が説明していない変更の一覧。
///
/// 台本の出来に依らず全ての変更に行き着けることが要るので、数えるのはホスト側。AI に
/// 「漏らしたものを申告して」と頼む形にすると、漏らした自覚が無いときに何も出ない。
pub fn unexplained(tour: &Tour, changed: &[FileDiff]) -> Vec<Place> {
    let mut out = Vec::new();
    for file in changed {
        let mut has_lines = false;
        for side in [Side::New, Side::Old] {
            let changed_lines = lines_on(file, side);
            has_lines |= !changed_lines.is_empty();
            let missed = changed_lines
                .into_iter()
                .filter(|line| !tour.places().any(|p| p.covers(&file.path, side, *line)));
            out.extend(runs(missed).into_iter().map(|(start, end)| Place {
                path: file.path.clone(),
                side,
                start: Some(start),
                end: Some(end),
            }));
        }
        // バイナリとモードだけの変更は指せる行を持たない。ファイルごと1つ出す。
        if !has_lines && !tour.places().any(|place| place.path == file.path) {
            out.push(Place::whole(&file.path));
        }
    }
    out
}

/// 存在しない行を指している場所の (停留所, 場所) 番号。
///
/// AI に書かせた台本は行番号を作り話する。指した先に何も無いのを黙って開くと、無関係な
/// コードを「ここが中核」として読ませることになる。
///
/// 前像は作業ツリーに無いので後像だけ見る。削除されたファイルを指す前像の場所は正しい。
pub fn misplaced(tour: &Tour, worktree: &Path) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut counted: HashMap<&str, Option<u32>> = Default::default();
    for (s, stop) in tour.stops.iter().enumerate() {
        for (p, place) in stop.places.iter().enumerate() {
            if place.side == Side::Old {
                continue;
            }
            let Some(last) = *counted
                .entry(place.path.as_str())
                .or_insert_with(|| line_count(&worktree.join(&place.path)))
            else {
                out.push((s, p));
                continue;
            };
            if place
                .start
                .into_iter()
                .chain(place.end)
                .any(|line| line > last)
            {
                out.push((s, p));
            }
        }
    }
    out
}

fn line_count(path: &Path) -> Option<u32> {
    let text = std::fs::read_to_string(path).ok()?;
    Some(text.lines().count() as u32)
}

fn lines_on(file: &FileDiff, side: Side) -> Vec<u32> {
    let want = match side {
        Side::New => DiffLineTag::Insert,
        Side::Old => DiffLineTag::Delete,
    };
    file.hunks
        .iter()
        .flat_map(|hunk| &hunk.lines)
        .filter(|line| line.tag == want)
        .filter_map(|line| match side {
            Side::New => line.new_line_no,
            Side::Old => line.old_line_no,
        })
        .map(|line| line as u32)
        .collect()
}

fn runs(lines: impl Iterator<Item = u32>) -> Vec<(u32, u32)> {
    let mut lines: Vec<u32> = lines.collect();
    lines.sort_unstable();
    lines.dedup();
    let mut out: Vec<(u32, u32)> = Vec::new();
    for line in lines {
        match out.last_mut() {
            Some((_, end)) if line <= *end + GAP => *end = line,
            _ => out.push((line, line)),
        }
    }
    out
}

/// [load] の結果。「無い」と「壊れている」を畳まない。走らせていないだけの状態と、
/// 直せる異常を同じ顔で出すと、どちらも黙って無視されるようになる。
#[derive(Debug)]
pub enum Outcome {
    Missing,
    Loaded(Box<Tour>),
    Broken(String),
}

pub fn load(worktree: &Path) -> Outcome {
    // 相対パスのまま読むと、根が決まっていないときに process の cwd にある別の台本を開く。
    if !worktree.is_absolute() {
        return Outcome::Missing;
    }
    let path = artifact_path(worktree);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Outcome::Missing,
        Err(e) => return Outcome::Broken(format!("{}: {e}", path.display())),
    };
    match Tour::from_json(&text) {
        Ok(tour) => Outcome::Loaded(Box::new(tour)),
        Err(e) => Outcome::Broken(format!("{}: {e}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff_state::{DiffHunk, DiffLine, FileStatus};

    const SCRIPT: &str = r#"{
      "schema_version": 1,
      "title": "キー入力の行き先",
      "stops": [
        {
          "title": "キーを計画ビューへ回す",
          "narration": "メニューとモーダルより後、PTY 転送より前に挟む。",
          "places": [
            { "path": "src/route.rs", "start": 40, "end": 49 },
            { "path": "src/workspace.rs" }
          ]
        }
      ]
    }"#;

    #[test]
    fn 台本を読むと停留所と場所が並ぶ() {
        let tour = Tour::from_json(SCRIPT).unwrap();
        assert_eq!(tour.title, "キー入力の行き先");
        let [stop] = tour.stops.as_slice() else {
            panic!("{:?}", tour.stops);
        };
        assert_eq!(stop.places.len(), 2);
        assert_eq!(
            (stop.places[0].start, stop.places[0].end),
            (Some(40), Some(49))
        );
        assert_eq!(
            (stop.places[1].start, stop.places[1].end),
            (None, None),
            "範囲は省ける"
        );
    }

    #[test]
    fn 版が違う台本は読まずに理由を返す() {
        let text = SCRIPT.replace("\"schema_version\": 1", "\"schema_version\": 99");
        let err = Tour::from_json(&text).unwrap_err();
        assert!(err.contains("99"), "{err}");
    }

    fn file(path: &str, added: &[usize], deleted: &[usize]) -> FileDiff {
        let lines = added
            .iter()
            .map(|n| DiffLine {
                tag: DiffLineTag::Insert,
                old_line_no: None,
                new_line_no: Some(*n),
                inline_segments: Vec::new(),
                content: String::new(),
            })
            .chain(deleted.iter().map(|n| DiffLine {
                tag: DiffLineTag::Delete,
                old_line_no: Some(*n),
                new_line_no: None,
                inline_segments: Vec::new(),
                content: String::new(),
            }))
            .collect();
        FileDiff {
            path: path.to_string(),
            status: FileStatus::Modified,
            added_lines: added.len(),
            deleted_lines: deleted.len(),
            hunks: vec![DiffHunk {
                lines,
                func_header: None,
            }],
        }
    }

    fn tour(places: Vec<Place>) -> Tour {
        Tour {
            schema_version: SCHEMA_VERSION,
            title: String::new(),
            stops: vec![Stop {
                title: "s".into(),
                narration: String::new(),
                places,
            }],
        }
    }

    fn at(path: &str, side: Side, start: u32, end: u32) -> Place {
        Place {
            path: path.into(),
            side,
            start: Some(start),
            end: Some(end),
        }
    }

    #[test]
    fn 説明されていない行だけが範囲にまとまる() {
        let tour = tour(vec![at("a.rs", Side::New, 10, 12)]);
        let changed = [file("a.rs", &[10, 11, 12, 20, 21, 40], &[])];

        assert_eq!(
            unexplained(&tour, &changed),
            [at("a.rs", Side::New, 20, 21), at("a.rs", Side::New, 40, 40)],
            "10-12 は説明済み、20-21 は隣り合うので1つ、40 は離れているので別"
        );
    }

    #[test]
    fn 追加行を説明しても削除行は別に数える() {
        let tour = tour(vec![at("a.rs", Side::New, 10, 12)]);
        let changed = [file("a.rs", &[10, 11, 12], &[30, 31])];

        assert_eq!(
            unexplained(&tour, &changed),
            [at("a.rs", Side::Old, 30, 31)],
            "後像を説明しても前像は説明したことにならない"
        );
    }

    #[test]
    fn 前像を名指しした場所は削除行を説明する() {
        let tour = tour(vec![
            at("a.rs", Side::New, 10, 12),
            at("a.rs", Side::Old, 30, 31),
        ]);
        let changed = [file("a.rs", &[10, 11, 12], &[30, 31])];
        assert!(unexplained(&tour, &changed).is_empty());
    }

    #[test]
    fn 同じ行番号でも側が違えば別の行として数える() {
        let tour = tour(vec![at("a.rs", Side::Old, 5, 5)]);
        let changed = [file("a.rs", &[5], &[5])];

        assert_eq!(
            unexplained(&tour, &changed),
            [at("a.rs", Side::New, 5, 5)],
            "前像の 5 を説明しても後像の 5 は残る"
        );
    }

    #[test]
    fn 削除だけのファイルは前像の行で出る() {
        let changed = [file("gone.rs", &[], &[1, 2, 3])];

        assert_eq!(
            unexplained(&tour(Vec::new()), &changed),
            [at("gone.rs", Side::Old, 1, 3)]
        );

        let touched = tour(vec![Place::whole("gone.rs")]);
        assert!(
            unexplained(&touched, &changed).is_empty(),
            "ファイル全体を指す場所が既にあれば出さない"
        );
    }

    #[test]
    fn 指せる行が無い変更はファイルごと出る() {
        let binary = FileDiff {
            path: "logo.png".into(),
            status: FileStatus::Modified,
            added_lines: 0,
            deleted_lines: 0,
            hunks: Vec::new(),
        };

        assert_eq!(
            unexplained(&tour(Vec::new()), &[binary.clone()]),
            [Place::whole("logo.png")]
        );
        assert!(
            unexplained(&tour(vec![Place::whole("logo.png")]), &[binary]).is_empty(),
            "台本が触れていれば出さない"
        );
    }

    #[test]
    fn 範囲を持たない場所はそのファイルの全行を説明したことになる() {
        let tour = tour(vec![Place::whole("a.rs")]);
        let changed = [file("a.rs", &[1, 500], &[])];
        assert!(unexplained(&tour, &changed).is_empty());
    }

    #[test]
    fn 存在しない行と無いファイルを指す場所を名指しする() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "one\ntwo\nthree\n").unwrap();
        let tour = tour(vec![
            at("a.rs", Side::New, 1, 3),
            at("a.rs", Side::New, 3, 4),
            at("a.rs", Side::Old, 900, 900),
            at("gone.rs", Side::New, 1, 1),
            at("gone.rs", Side::Old, 1, 1),
        ]);

        assert_eq!(
            misplaced(&tour, dir.path()),
            [(0, 1), (0, 3)],
            "範囲外と無いファイルだけ。前像は作業ツリーに無いので見ない"
        );
    }

    #[test]
    fn 根が決まっていなければ何も読まない() {
        assert!(matches!(load(Path::new("")), Outcome::Missing));
        assert!(matches!(load(Path::new("sub/dir")), Outcome::Missing));
    }

    #[test]
    fn 台本が無いのと壊れているのは別の結果になる() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(load(dir.path()), Outcome::Missing));

        std::fs::create_dir_all(dir.path().join(".conductor")).unwrap();
        std::fs::write(artifact_path(dir.path()), "{ not json").unwrap();
        assert!(matches!(load(dir.path()), Outcome::Broken(_)));
    }
}
