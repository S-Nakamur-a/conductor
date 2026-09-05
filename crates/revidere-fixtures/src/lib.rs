// テストが使う JSON の組み立て。
//
// schema・base/head・overview・coverage は検証の差分にならないのに、呼ぶ側で
// 書き写すとスキーマ版が古いまま残る。骨組みはここへ集めて、差分になる
// 部分だけを渡してもらう。

use std::fmt::Write as _;

/// 項目 1 つの JSON。壊れた項目も組めるよう、重要度と理由は生の値で持つ。
#[derive(Clone)]
pub struct Section {
    title: String,
    importance: String,
    reason: Option<String>,
    ranges: Vec<String>,
    relations: Vec<String>,
}

impl Section {
    pub fn new(title: &str, importance: &str) -> Self {
        Self {
            title: title.into(),
            importance: importance.into(),
            reason: Some("r".into()),
            ranges: Vec::new(),
            relations: Vec::new(),
        }
    }

    pub fn reason(mut self, reason: Option<&str>) -> Self {
        self.reason = reason.map(str::to_string);
        self
    }

    /// 行の範囲。end に None を渡すと start だけの範囲になる。
    pub fn lines(mut self, path: &str, side: &str, start: u32, end: Option<u32>) -> Self {
        let mut r = format!(r#"{{"path":"{path}","side":"{side}","start":{start}"#);
        if let Some(e) = end {
            let _ = write!(r, r#","end":{e}"#);
        }
        r.push('}');
        self.ranges.push(r);
        self
    }

    pub fn line(self, path: &str, side: &str, line: u32) -> Self {
        self.lines(path, side, line, Some(line))
    }

    /// 行を持たないファイル単位の範囲。
    pub fn whole_file(mut self, path: &str) -> Self {
        self.ranges
            .push(format!(r#"{{"path":"{path}","side":"file"}}"#));
        self
    }

    pub fn relation(mut self, to: &str, primary: bool) -> Self {
        self.relations.push(format!(
            r#"{{"to":"{to}","reason":"r","primary":{primary}}}"#
        ));
        self
    }

    pub fn json(&self) -> String {
        let reason = match &self.reason {
            Some(r) => format!(r#","reason":"{r}""#),
            None => String::new(),
        };
        format!(
            r#"{{"title":"{}","body":"b","importance":"{}"{reason},"ranges":[{}],"relations":[{}]}}"#,
            self.title,
            self.importance,
            self.ranges.join(","),
            self.relations.join(",")
        )
    }
}

pub fn sections(items: &[Section]) -> String {
    let body: Vec<String> = items.iter().map(Section::json).collect();
    format!("[{}]", body.join(","))
}

/// 成果物 JSON。
pub fn review(sections_json: &str) -> String {
    review_with_impacts(sections_json, "[]")
}

pub fn review_with_impacts(sections_json: &str, impacts_json: &str) -> String {
    format!(
        r#"{{
  "schema": {schema}, "base": "aaa", "head": "bbb",
  "overview": {OVERVIEW},
  "sections": {sections_json},
  "impacts": {impacts_json},
  "coverage": {{"total":2,"classified":2,"unclassified":[],"conflicts":[],"unknown":[]}}
}}"#,
        schema = revidere::review::SCHEMA_VERSION,
    )
}

/// モデルの応答 JSON。schema も coverage も持たない。
pub fn answer(sections_json: &str) -> String {
    answer_with_impacts(sections_json, "[]")
}

pub fn answer_with_impacts(sections_json: &str, impacts_json: &str) -> String {
    format!(r#"{{"overview":{OVERVIEW},"sections":{sections_json},"impacts":{impacts_json}}}"#)
}

const OVERVIEW: &str =
    r#"{"problem":"p","change":"c","mechanism":"m","placement":"pl","scope":"s"}"#;

/// テストごとに使い捨てる、実物の git リポジトリ。
///
/// git の呼び方そのものが正しさなので、モックではなく実物を子プロセスとして
/// 動かして確かめる。
pub struct Repo {
    dir: std::path::PathBuf,
}

impl Repo {
    pub fn new(tag: &str) -> Self {
        use std::sync::atomic::{AtomicU32, Ordering};
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("revidere-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let repo = Repo { dir };
        repo.git(&["init", "-q", "-b", "main"]);
        repo.git(&["config", "user.email", "t@example.com"]);
        repo.git(&["config", "user.name", "t"]);
        repo
    }

    pub fn dir(&self) -> &std::path::Path {
        &self.dir
    }

    pub fn git(&self, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(&self.dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?} が失敗した: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).to_string()
    }

    pub fn write(&self, path: &str, content: &str) {
        let p = self.dir.join(path);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(p, content).unwrap();
    }

    /// 変更を全部コミットして、そのコミットの oid を返す。
    pub fn commit_all(&self, msg: &str) -> String {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "-m", msg]);
        self.head()
    }

    pub fn branch(&self, name: &str) {
        self.git(&["checkout", "-q", "-b", name]);
    }

    pub fn head(&self) -> String {
        self.git(&["rev-parse", "HEAD"]).trim().to_string()
    }

    /// レビューの起点。実際の呼ばれ方どおり merge-base を経由する。
    pub fn merge_base(&self, base: &str) -> String {
        revidere::git::merge_base(&self.dir, base).unwrap()
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
