//! 索引の生成を 1 行 1 件で追記する記録。
//!
//! 生成はバックグラウンドで起き、終わったことしか画面に出ない。あとから
//! 「何をきっかけに、どこを、どれだけかけて作ったか」と「その生成に意味が
//! あったか」を追えるようにするためのもので、producer 自身の出力
//! (`index.<lang>.log`、生成ごとに上書き) とは別物。
//!
//! 書式は `key=value` の並び。桁を揃えた表にすると、値が伸びたときに
//! grep の当て先が動く。

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::Path;
use std::time::Duration;

/// 記録の上限。超えたら古い半分を捨てる。1 行 200 バイト程度なので数千件は残る。
/// リポジトリの中に置きっぱなしになるファイルなので、際限なく伸びる形にはしない。
const MAX_BYTES: u64 = 512 * 1024;

/// 生成が始まったきっかけ。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Trigger {
    /// Viewer で開いたファイルの索引ルートに、索引がまだ無かった。
    Open,
    /// そのルートの中のファイルが変わった。
    Change,
    /// 画面から作り直しを頼まれた。
    Manual,
    /// `conductor index`。
    Cli,
}

impl Trigger {
    fn tag(self) -> &'static str {
        match self {
            Trigger::Open => "open",
            Trigger::Change => "change",
            Trigger::Manual => "manual",
            Trigger::Cli => "cli",
        }
    }
}

/// 前の生成から、この producer が読むファイルがどれだけ動いたか。
///
/// 出自の表には索引ルートの全ファイルが載る (`.md` や `.json` も) ので、
/// その言語のファイルだけに絞って数える。絞らないと、README を直しただけの
/// 生成が「ソースが動いた」に見えて、無駄だったことが分からなくなる。
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct SourceDelta {
    pub added: usize,
    pub modified: usize,
    pub removed: usize,
}

impl SourceDelta {
    pub fn is_empty(self) -> bool {
        self == SourceDelta::default()
    }
}

/// 出自の表を比べられたか。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sources {
    /// 前の索引が無い。比べる相手がいない。
    First,
    /// 比べた結果。空なら、この生成は前と同じものを作り直しただけ。
    Delta(SourceDelta),
    /// 比べていない (生成に至らなかった)。
    Unknown,
}

/// 生成 1 件の顛末。
pub struct Entry<'a> {
    /// 索引ルート。ツリーのルート自身なら `.` と書く。
    pub root: &'a Path,
    pub lang: &'a str,
    pub trigger: Trigger,
    /// きっかけになったファイル。ツリーのルートからの相対パス。
    pub cause: Option<&'a Path>,
    /// 頼まれてから producer が立つまで。静穏時間の待ちとロック待ちの合計。
    pub waited: Duration,
    /// producer が立ってから投入し終わるまで。
    pub took: Duration,
    pub outcome: Outcome<'a>,
    pub sources: Sources,
    /// 生成中に変わったファイルの数。0 でなければ、その索引は置いた時点で既に古い。
    pub changed_during: usize,
}

pub enum Outcome<'a> {
    Ready {
        documents: usize,
    },
    Failed(&'a str),
    /// ほかが生成中でロックを取れなかった。待機に戻ってやり直す。
    Busy,
    /// producer を起動できない。以後試みない。
    Unavailable(&'a str),
    /// 対象のツリーが変わったので捨てた。ここまでの producer の時間は無駄になる。
    Aborted,
}

/// 1 件書く。書けなくても何もしない (記録が取れないことで生成を止めない)。
pub fn append(dir: &Path, entry: &Entry<'_>) {
    let path = dir.join("index-history.log");
    truncate_if_large(&path);

    let root = entry.root.to_string_lossy();
    let mut line = format!("{} lang={} root={}", stamp(), entry.lang, spell(&root));
    let _ = write!(line, " trigger={}", entry.trigger.tag());
    if let Some(cause) = entry.cause {
        let _ = write!(line, " cause={}", spell(&cause.to_string_lossy()));
    }
    let _ = write!(
        line,
        " waited={} took={} ",
        secs(entry.waited),
        secs(entry.took)
    );
    match &entry.outcome {
        Outcome::Ready { documents } => {
            let _ = write!(line, "result=ok documents={documents}");
        }
        Outcome::Failed(why) => {
            let _ = write!(line, "result=failed reason={}", one_line(why));
        }
        Outcome::Busy => line.push_str("result=busy"),
        Outcome::Unavailable(why) => {
            let _ = write!(line, "result=unavailable reason={}", one_line(why));
        }
        Outcome::Aborted => line.push_str("result=aborted"),
    }

    let mut waste: Vec<String> = Vec::new();
    match entry.sources {
        Sources::First => line.push_str(" sources=first"),
        Sources::Delta(d) if d.is_empty() => {
            // 前の索引と同じソースから作り直しただけ。この生成は要らなかった。
            line.push_str(" sources=none");
            waste.push("no-source-change".into());
        }
        Sources::Delta(d) => {
            let _ = write!(line, " sources=+{}~{}-{}", d.added, d.modified, d.removed);
        }
        Sources::Unknown => {}
    }
    if entry.changed_during > 0 {
        waste.push(format!("stale-on-arrival({} files)", entry.changed_during));
    }
    if matches!(entry.outcome, Outcome::Aborted) {
        waste.push("discarded".into());
    }
    if !waste.is_empty() {
        let _ = write!(line, " waste={}", waste.join(","));
    }
    line.push('\n');

    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| f.write_all(line.as_bytes()));
}

/// 空のパスは `.`、空白を含むパスは引用する。`key=value` の並びが崩れないように。
fn spell(path: &str) -> String {
    if path.is_empty() {
        return ".".into();
    }
    if path.contains(char::is_whitespace) {
        return format!("\"{path}\"");
    }
    path.to_string()
}

fn one_line(why: &str) -> String {
    let flat = why.replace('\n', " ");
    if flat.contains(char::is_whitespace) {
        format!("\"{flat}\"")
    } else {
        flat
    }
}

fn secs(d: Duration) -> String {
    format!("{:.1}s", d.as_secs_f64())
}

/// UTC の秒までの時刻。時刻の表示のためだけに依存を増やさない。
fn stamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64;
    let (days, rest) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// 1970-01-01 からの日数を暦の日付にする (Howard Hinnant の civil_from_days)。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (era * 400 + yoe + i64::from(m <= 2), m, d)
}

/// 大きくなりすぎた記録の古い半分を捨てる。
fn truncate_if_large(path: &Path) {
    if std::fs::metadata(path).is_ok_and(|m| m.len() <= MAX_BYTES) {
        return;
    }
    let Ok(content) = std::fs::read_to_string(path) else {
        return;
    };
    let lines: Vec<&str> = content.lines().collect();
    let kept = lines[lines.len() / 2..].join("\n");
    let _ = std::fs::write(path, format!("{kept}\n"));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry<'a>(outcome: Outcome<'a>, sources: Sources) -> Entry<'a> {
        Entry {
            root: Path::new("services/api"),
            lang: "go",
            trigger: Trigger::Change,
            cause: Some(Path::new("services/api/handler/handler.go")),
            waited: Duration::from_millis(3_100),
            took: Duration::from_millis(1_800),
            outcome,
            sources,
            changed_during: 0,
        }
    }

    fn written(dir: &tempfile::TempDir) -> String {
        std::fs::read_to_string(dir.path().join("index-history.log")).unwrap()
    }

    #[test]
    fn 何をきっかけにどこを作ったかを残す() {
        let dir = tempfile::tempdir().unwrap();
        append(
            dir.path(),
            &entry(
                Outcome::Ready { documents: 42 },
                Sources::Delta(SourceDelta {
                    modified: 1,
                    ..Default::default()
                }),
            ),
        );

        let log = written(&dir);
        assert!(log.contains("lang=go root=services/api"), "{log}");
        assert!(log.contains("trigger=change"), "{log}");
        assert!(
            log.contains("cause=services/api/handler/handler.go"),
            "{log}"
        );
        assert!(log.contains("waited=3.1s took=1.8s"), "{log}");
        assert!(log.contains("result=ok documents=42"), "{log}");
        assert!(log.contains("sources=+0~1-0"), "{log}");
        assert!(!log.contains("waste="), "動いたのに無駄と書いた: {log}");
    }

    #[test]
    fn ソースが動いていない生成は無駄として残す() {
        // これが分からないと、要らなかった生成を数えられない。
        let dir = tempfile::tempdir().unwrap();
        append(
            dir.path(),
            &entry(
                Outcome::Ready { documents: 42 },
                Sources::Delta(SourceDelta::default()),
            ),
        );

        let log = written(&dir);
        assert!(log.contains("sources=none"), "{log}");
        assert!(log.contains("waste=no-source-change"), "{log}");
    }

    #[test]
    fn 初回は比べる相手がいないので無駄と言わない() {
        let dir = tempfile::tempdir().unwrap();
        append(
            dir.path(),
            &entry(Outcome::Ready { documents: 42 }, Sources::First),
        );

        let log = written(&dir);
        assert!(log.contains("sources=first"), "{log}");
        assert!(!log.contains("waste="), "{log}");
    }

    #[test]
    fn 無駄の理由は並べて書く() {
        let dir = tempfile::tempdir().unwrap();
        let mut e = entry(
            Outcome::Ready { documents: 1 },
            Sources::Delta(SourceDelta::default()),
        );
        e.changed_during = 2;
        append(dir.path(), &e);

        let log = written(&dir);
        assert!(
            log.contains("waste=no-source-change,stale-on-arrival(2 files)"),
            "{log}"
        );
    }

    #[test]
    fn 捨てた生成も無駄として残す() {
        // worktree を切り替えると、そこまでの producer の時間は捨てられる。
        let dir = tempfile::tempdir().unwrap();
        append(dir.path(), &entry(Outcome::Aborted, Sources::Unknown));

        let log = written(&dir);
        assert!(log.contains("result=aborted"), "{log}");
        assert!(log.contains("waste=discarded"), "{log}");
        assert!(!log.contains("sources="), "比べていないのに書いた: {log}");
    }

    #[test]
    fn 失敗の理由は_1_行に畳んで引用する() {
        // 1 件 1 行という読み方も、key=value という読み方も崩さない。
        let dir = tempfile::tempdir().unwrap();
        append(
            dir.path(),
            &entry(Outcome::Failed("落ちた\n詳細は log"), Sources::Unknown),
        );

        let log = written(&dir);
        assert_eq!(log.lines().count(), 1, "{log}");
        assert!(
            log.contains(r#"result=failed reason="落ちた 詳細は log""#),
            "{log}"
        );
    }

    #[test]
    fn ツリーのルート自身は空ではなくドットで書く() {
        let dir = tempfile::tempdir().unwrap();
        let mut e = entry(Outcome::Busy, Sources::Unknown);
        e.root = Path::new("");
        e.cause = None;
        append(dir.path(), &e);

        let log = written(&dir);
        assert!(log.contains("root=. trigger=change waited="), "{log}");
    }

    #[test]
    fn 追記されるので前の行が残る() {
        let dir = tempfile::tempdir().unwrap();
        append(dir.path(), &entry(Outcome::Busy, Sources::Unknown));
        append(dir.path(), &entry(Outcome::Aborted, Sources::Unknown));

        assert_eq!(written(&dir).lines().count(), 2);
    }

    #[test]
    fn 時刻は_utc_の_iso8601() {
        let at = stamp();
        assert_eq!(at.len(), 20, "{at}");
        assert!(at.ends_with('Z'), "{at}");
        // 桁位置がずれると、あとで並べ替えたときに壊れる。
        assert_eq!(&at[4..5], "-");
        assert_eq!(&at[10..11], "T");
    }

    #[test]
    fn 暦の変換が既知の日付と合う() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        // 閏日。ここを外すと 1 日ずれた記録が残る。
        assert_eq!(civil_from_days(19_783), (2024, 3, 1));
    }
}
