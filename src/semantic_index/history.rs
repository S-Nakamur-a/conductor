//! 索引の生成を 1 行 1 件で追記する記録。
//!
//! 生成はバックグラウンドで起き、終わったことしか画面に出ない。あとから
//! 「いつ・何をきっかけに・どこを・どれだけかけて作ったか」と「無駄が無かったか」を
//! 追えるようにするための記録で、producer 自身の出力 (`index.<lang>.log`、
//! 生成ごとに上書き) とは別物。

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::Path;
use std::time::Duration;

/// 記録の上限。超えたら古い半分を捨てる。1 行 100 バイト程度なので、
/// 数千件は残る。リポジトリの中に置きっぱなしになるファイルなので、
/// 際限なく伸びる形にはしない。
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

/// 生成 1 件の顛末。
pub struct Entry<'a> {
    /// 索引ルート。ツリーのルート自身なら `.` と書く。
    pub root: &'a Path,
    pub lang: &'a str,
    pub trigger: Trigger,
    /// 頼まれてから producer が立つまで。静穏時間の待ちとロック待ちの合計。
    pub waited: Duration,
    /// producer が立ってから投入し終わるまで。
    pub took: Duration,
    /// 結果。成功なら Document 数、そうでなければ理由。
    pub outcome: Outcome<'a>,
    /// 生成中に来た変更の数。0 でなければ、この索引は置いた時点で既に古い。
    pub restarts: usize,
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
    let root = if root.is_empty() { "." } else { &root };
    let mut line = String::new();
    let _ = write!(
        line,
        "{} {:<4} {:<24} trigger={:<6} waited={:>6} took={:>7} ",
        stamp(),
        entry.lang,
        root,
        entry.trigger.tag(),
        secs(entry.waited),
        secs(entry.took),
    );
    match &entry.outcome {
        Outcome::Ready { documents } => {
            let _ = write!(line, "ok documents={documents}");
        }
        Outcome::Failed(why) => {
            let _ = write!(line, "failed {}", one_line(why));
        }
        Outcome::Busy => line.push_str("busy"),
        Outcome::Unavailable(why) => {
            let _ = write!(line, "unavailable {}", one_line(why));
        }
        Outcome::Aborted => line.push_str("aborted"),
    }
    if entry.restarts > 0 {
        // 生成中に入った変更。この世代の索引は、置いた時点で既にその変更を含まない。
        let _ = write!(line, " stale-on-arrival changes={}", entry.restarts);
    }
    line.push('\n');

    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| f.write_all(line.as_bytes()));
}

fn one_line(why: &str) -> String {
    why.replace('\n', " ")
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

    fn entry<'a>(outcome: Outcome<'a>, restarts: usize) -> Entry<'a> {
        Entry {
            root: Path::new("services/api"),
            lang: "go",
            trigger: Trigger::Change,
            waited: Duration::from_millis(3_100),
            took: Duration::from_millis(1_800),
            outcome,
            restarts,
        }
    }

    #[test]
    fn 成功した生成は所要時間と_document_数を残す() {
        let dir = tempfile::tempdir().unwrap();
        append(dir.path(), &entry(Outcome::Ready { documents: 42 }, 0));

        let log = std::fs::read_to_string(dir.path().join("index-history.log")).unwrap();
        assert!(log.contains("go"), "{log}");
        assert!(log.contains("services/api"), "{log}");
        assert!(log.contains("trigger=change"), "{log}");
        assert!(log.contains("waited=  3.1s"), "{log}");
        assert!(log.contains("took=   1.8s"), "{log}");
        assert!(log.contains("ok documents=42"), "{log}");
    }

    #[test]
    fn 生成中に来た変更を無駄として残す() {
        // これが付いた行は、置いた時点で既に古い索引を作っている。
        let dir = tempfile::tempdir().unwrap();
        append(dir.path(), &entry(Outcome::Ready { documents: 1 }, 2));

        let log = std::fs::read_to_string(dir.path().join("index-history.log")).unwrap();
        assert!(log.contains("stale-on-arrival changes=2"), "{log}");
    }

    #[test]
    fn 失敗の理由は_1_行に畳む() {
        // 1 件 1 行という読み方が崩れると、あとから追えなくなる。
        let dir = tempfile::tempdir().unwrap();
        append(dir.path(), &entry(Outcome::Failed("落ちた\n詳細は log"), 0));

        let log = std::fs::read_to_string(dir.path().join("index-history.log")).unwrap();
        assert_eq!(log.lines().count(), 1, "{log}");
        assert!(log.contains("failed 落ちた 詳細は log"), "{log}");
    }

    #[test]
    fn 追記されるので前の行が残る() {
        let dir = tempfile::tempdir().unwrap();
        append(dir.path(), &entry(Outcome::Busy, 0));
        append(dir.path(), &entry(Outcome::Aborted, 0));

        let log = std::fs::read_to_string(dir.path().join("index-history.log")).unwrap();
        assert_eq!(log.lines().count(), 2, "{log}");
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
