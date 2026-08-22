//! 索引の出自。どのファイルが索引生成時のままかを申告する表の、採取と読み書き。
//!
//! producer は起動してからしばらくソースを読み続けるので、ハッシュは生成の前後で
//! 2 回取り、両方一致したファイルだけを申告する。

use super::Producer;
use crate::blob_hash;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// 索引を作った道具の素性。出自の表の見出しに書いて、投入時に照合する。
///
/// 出力先だけは生成ごとに変わる（一時ファイル名に PID が入る）ので、置き場所を
/// 名前に置き換える。索引の中身は出力先に依らない。
fn fingerprint(producer: &dyn Producer) -> String {
    producer.command(Path::new("<out>")).join(" ")
}

/// 出自の表を読む。[`Store::load`] の `expected` にそのまま渡せる。
///
/// `producer` が作ったのではない表は読まない。道具が変われば同じソースから別の索引が
/// 出る（`scip-typescript` は解決した `typescript` の版をシンボルの綴りに埋める）ので、
/// 前の道具の索引を使い続けると、いま作れるものとは違う答えを返し続けることになる。
///
/// 見出しの無い表も読まない。古い書式を読む分岐を置くと、その分岐が
/// 「照合していない状態」を静かに残す。
pub fn read_provenance(path: &Path, producer: &dyn Producer) -> Option<HashMap<PathBuf, String>> {
    let content = std::fs::read_to_string(path).ok()?;
    let (header, body) = content.split_once('\n')?;
    if header.strip_prefix("# producer ")? != fingerprint(producer) {
        return None;
    }
    let mut expected = HashMap::new();
    for line in body.lines() {
        // ハッシュは空白を含まないので、最初の空白で切ればパス側の空白は残る。
        // 壊れた行は個別に読み飛ばす。
        if let Some((hash, rel)) = line.split_once(' ') {
            expected.insert(PathBuf::from(rel), hash.to_string());
        }
    }
    Some(expected)
}

/// ファイルの内容ハッシュと更新時刻。
type Snapshot = HashMap<PathBuf, (String, Option<SystemTime>)>;

/// ツリーの全ファイルの内容ハッシュと更新時刻を取る。実測で 392 ファイル 30ms。
///
/// 追跡されていないファイルも対象に入る。索引はそれらの Document を持つので、
/// 対象から外すと出自を言えない Document が残る。
///
/// `require_git(false)` は既定ではない。指定しないと、git 管理下でないツリーで
/// `.gitignore` が無視され、ビルド成果物まで読んでハッシュを取ってしまう。
pub(super) fn snapshot(root: &Path) -> Snapshot {
    let mut out = Snapshot::new();
    for entry in ignore::WalkBuilder::new(root)
        .require_git(false)
        .build()
        .flatten()
    {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let Ok(rel) = entry.path().strip_prefix(root) else {
            continue;
        };
        let Ok(content) = std::fs::read(entry.path()) else {
            continue;
        };
        let mtime = entry.metadata().ok().and_then(|m| m.modified().ok());
        out.insert(rel.to_path_buf(), (blob_hash(&content), mtime));
    }
    out
}

/// 生成をまたいで内容も更新時刻も動かなかったファイルだけを残す。
///
/// 内容だけを見ると、生成中に書き換えて元に戻した場合にすり抜ける。索引は
/// 途中の内容を記述しているのに、こちらは元の内容を出自として申告してしまう。
pub(super) fn unchanged(before: &Snapshot, after: &Snapshot) -> HashMap<PathBuf, String> {
    before
        .iter()
        .filter(|(path, first)| after.get(*path).is_some_and(|second| second == *first))
        .map(|(path, (hash, _))| (path.clone(), hash.clone()))
        .collect()
}

/// 出自の表を書く。索引を置いたあとにだけ呼ぶ。
///
/// 形式は 1 行 1 ファイルで「40 桁のハッシュ、空白、相対パス」。ハッシュの幅が
/// 固定なので、空白を含むパスもそのまま書ける。
///
/// 公開しているのは、索引と表の組を自前で用意する検査を組み込む側が書けるようにするため。
/// 形式を向こうに書き写させると、[`read_provenance`] の側だけが変わったときに気づけない。
pub fn write_provenance(
    path: &Path,
    producer: &dyn Producer,
    expected: &HashMap<PathBuf, String>,
) -> std::io::Result<()> {
    let mut body = format!("# producer {}\n", fingerprint(producer));
    for (rel, hash) in expected {
        let Some(spelled) = rel.to_str() else {
            continue;
        };
        // 行で区切るので、改行を含むパスは表現できない。落としても Exact の
        // 対象から外れるだけで、誤った答えにはならない。
        if spelled.contains('\n') {
            continue;
        }
        body.push_str(hash);
        body.push(' ');
        body.push_str(spelled);
        body.push('\n');
    }
    std::fs::write(path, body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RustAnalyzer, ScipGo};
    use std::time::Duration;

    fn snap(entries: &[(&str, &str, u64)]) -> Snapshot {
        entries
            .iter()
            .map(|(path, hash, secs)| {
                (
                    PathBuf::from(path),
                    (
                        hash.to_string(),
                        Some(SystemTime::UNIX_EPOCH + Duration::from_secs(*secs)),
                    ),
                )
            })
            .collect()
    }

    #[test]
    fn 生成をまたいで動かなかったファイルだけが出自になる() {
        let before = snap(&[("a.rs", "h1", 10), ("b.rs", "h2", 10)]);
        let after = snap(&[("a.rs", "h1", 10), ("b.rs", "CHANGED", 20)]);
        let kept = unchanged(&before, &after);
        assert_eq!(kept.get(Path::new("a.rs")).map(String::as_str), Some("h1"));
        assert!(!kept.contains_key(Path::new("b.rs")));
    }

    #[test]
    fn 書き換えて元に戻したファイルは出自にしない() {
        // 内容だけを見ると素通りする。producer は途中の内容を読んでいるので、
        // 元の内容を出自として申告すると索引と食い違う。
        let before = snap(&[("a.rs", "h1", 10)]);
        let after = snap(&[("a.rs", "h1", 99)]);
        assert!(unchanged(&before, &after).is_empty());
    }

    #[test]
    fn 生成中に消えたファイルは出自にしない() {
        let before = snap(&[("a.rs", "h1", 10)]);
        assert!(unchanged(&before, &Snapshot::new()).is_empty());
    }

    #[test]
    fn 出自の表は固定幅のハッシュと相対パスで書く() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.hashes");
        let mut expected = HashMap::new();
        expected.insert(PathBuf::from("src/a b.rs"), "0".repeat(40));
        write_provenance(&path, &RustAnalyzer, &expected).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            format!(
                "# producer {}\n{} src/a b.rs\n",
                fingerprint(&RustAnalyzer),
                "0".repeat(40)
            )
        );
        // 書いたものがそのまま読み戻せる。
        assert_eq!(read_provenance(&path, &RustAnalyzer).unwrap(), expected);
    }

    #[test]
    fn 改行を含むパスは表から落とす() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.hashes");
        let mut expected = HashMap::new();
        expected.insert(PathBuf::from("src/a\nb.rs"), "0".repeat(40));
        write_provenance(&path, &RustAnalyzer, &expected).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            format!("# producer {}\n", fingerprint(&RustAnalyzer))
        );
    }

    #[test]
    fn 別の道具が作った出自の表は読まない() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.hashes");
        let mut expected = HashMap::new();
        expected.insert(PathBuf::from("src/a.rs"), "0".repeat(40));
        write_provenance(&path, &ScipGo, &expected).unwrap();

        assert!(read_provenance(&path, &ScipGo).is_some());
        assert!(read_provenance(&path, &RustAnalyzer).is_none());
    }

    #[test]
    fn 道具の名乗りが書き換わった出自の表は読まない() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.hashes");
        let mut expected = HashMap::new();
        expected.insert(PathBuf::from("src/a.rs"), "0".repeat(40));
        write_provenance(&path, &RustAnalyzer, &expected).unwrap();

        let tampered = std::fs::read_to_string(&path)
            .unwrap()
            .replace("--output", "--out");
        std::fs::write(&path, tampered).unwrap();

        assert!(read_provenance(&path, &RustAnalyzer).is_none());
    }

    #[test]
    fn 見出しの無い出自の表は読まない() {
        // 古い書式を読む分岐は置かない。索引の作り直しは Rust 13.8 秒 / Go 2.2 秒 /
        // TypeScript 11 秒なので、拒んで作り直させるほうが安い。
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.hashes");
        std::fs::write(&path, format!("{} src/a.rs\n", "0".repeat(40))).unwrap();

        assert!(read_provenance(&path, &RustAnalyzer).is_none());
    }

    #[test]
    fn 版を上げた_producer_の出自の表は読まない() {
        // 実際に起きるのはこの形。sheaf が固定している版を上げたら、
        // 前の版が作った索引は別の道具の産物になる。
        struct Pinned(&'static str);
        impl Producer for Pinned {
            fn command(&self, out: &Path) -> Vec<String> {
                vec![
                    "npx".into(),
                    format!("scip-typescript@{}", self.0),
                    "--output".into(),
                    out.to_string_lossy().into_owned(),
                ]
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.hashes");
        write_provenance(&path, &Pinned("0.4.0"), &HashMap::new()).unwrap();

        assert!(read_provenance(&path, &Pinned("0.4.0")).is_some());
        assert!(read_provenance(&path, &Pinned("0.5.0")).is_none());
    }

    #[test]
    fn 出自を取る対象は管理用ディレクトリとビルド成果物を含まない() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        // git 管理下でも gitignore が効くことを確かめる。git 管理下でない場合は
        // 別途 出自を取る対象は非_git_のツリーでもビルド成果物を含まない で見る。
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(root)
            .status()
            .unwrap();
        for (rel, body) in [
            ("src/a.rs", "fn a() {}\n"),
            (".sheaf/index.scip", "x\n"),
            ("target/debug/x", "x\n"),
            ("ignored.rs", "x\n"),
            (".gitignore", "/target\nignored.rs\n"),
        ] {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        }

        // .gitignore 自身も隠しファイルなので入らない。索引の Document になるのは
        // ソースだけなので、取りこぼしにはならない。
        let taken: Vec<String> = snapshot(root)
            .keys()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        assert_eq!(taken, vec!["src/a.rs".to_string()]);
    }

    #[test]
    fn 出自を取る対象は非_git_のツリーでもビルド成果物を含まない() {
        // ignore クレートの WalkBuilder は既定で require_git が true なので、
        // git 管理下でないツリーでは .gitignore を素通りする。sheaf 自身が
        // git 管理下に無い状態で使われることもあるため、ここが崩れると
        // ビルド成果物まで毎回ハッシュを取ることになる (V1 に効く)。
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for (rel, body) in [
            ("src/a.rs", "fn a() {}\n"),
            ("target/big.bin", "x\n"),
            (".gitignore", "/target\n"),
        ] {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        }

        let taken: Vec<String> = snapshot(root)
            .keys()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        assert_eq!(taken, vec!["src/a.rs".to_string()]);
    }
}
