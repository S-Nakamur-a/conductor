//! /clear によるセッションローテーションの追跡 — フォールバック経路。
//!
//! 正規の経路は SessionStart フック ([crate::cc_hook])。フックはパネル自身の
//! Claude プロセスの中で走り、新しい session id を持って来るので、対応は推測では
//! なく事実として決まる。このモジュールが使われるのはフックが沈黙したときだけ:
//! ユーザ設定でフックを無効にしている、claude が古くて --settings や
//! SessionStart を解さない、settings を書けなかった、など。
//!
//! Claude Code の /clear は会話を捨てるだけでなく、ログの書き込み先を新しい
//! session id の .jsonl に切り替える。新ファイルの先頭には /clear の
//! コマンドレコードが書かれ、以降の会話はすべてそちらに入る。旧ファイルには
//! 一切追記されず (実測で確認済み: clear 後は mtime が 1 ミリ秒も動かない)、
//! 両者を結ぶ id の相互参照もログに残らない。
//!
//! Conductor は起動時に --session-id で決め打ちした id を pin しているので、
//! そのままではローテーション後もずっと旧ファイルを読むことになる。ここでは
//! pin した id から「その続き」に当たるログへ連鎖を辿って、いま書かれている
//! ログの id を求める。フックが届いていれば pin 自体が更新済みなので、この
//! 連鎖は普通そのまま起点で止まる。
//!
//! ディレクトリ単位の推測に戻したわけではない点に注意。1 つの Claude
//! プロジェクトディレクトリにはそのワークツリーで走った全セッションのログが
//! 同居するので、「最新のログ」「後から始まったログ」といった条件では別の会話
//! を掴む。後続と認めるのは次を全部満たすログだけ:
//!
//!   1. 先頭の表示対象ユーザレコードが /clear コマンドである
//!      (新規に起動しただけの claude はこの形にならない)
//!   2. 開始時刻が前セッションの最終書き込み以降である
//!   3. 他の Claude パネルが pin している id ではない
//!
//! それでも曖昧さが残るのは、同一ワークツリーで複数の Claude パネルを開き、
//! その複数が /clear した場合だけ。そのときは開始時刻が前セッションの
//! 最終書き込みに最も近いものを採る。

use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, SystemTime};

use super::session_log_in_dir;

/// 前セッションの mtime に持たせる許容幅。
///
/// /clear の直前直後でどちらのファイルが先に書かれるかはミリ秒単位の話に
/// なるので、比較を厳密にすると取りこぼす。逆方向 (別セッションを誤って掴む)
/// のリスクはこの幅では実質増えない。
const MTIME_SLACK: Duration = Duration::from_secs(2);

/// 後続と認める、前セッションの最終書き込みからの間隔の上限。
///
/// ログだけを見ても「自分のセッションが /clear した」のと「同じワークツリー
/// で別の claude が起動して /clear した」のは区別できない。両者を分ける
/// 手がかりは間隔しかないので、そこで線を引く。
///
/// 実測 (手元の全プロジェクトのログ、/clear 始まりのログと直前のログの間隔)
/// では中央値が約 7 分、最短のものは 0〜9 秒。/clear は直前のターンが終わった
/// 直後に打たれるのが普通で、時間単位で離れているものは別の日に開いた別の実行
/// であることが多い。
///
/// これを超えたら連鎖を打ち切り、自分の clear 前のログを見せる。つまり
/// 「前のターンから 30 分以上空けてから /clear した」場合はこのバグの
/// 修正が効かず、以前と同じく clear 前が表示される。それでも、他人の会話を
/// 表示するよりは古い自分のものを見せる方がまし
/// (App::open_reflow に書いてある方針と同じ)。
const MAX_CLEAR_GAP: Duration = Duration::from_secs(1800);

/// ログの先頭から読み取る行数の上限。
///
/// /clear で始まるログではコマンドレコードは先頭数行に必ず入る
/// (mode / file-history-snapshot / caveat の後)。ここに無ければ
/// /clear 始まりではない。
const HEAD_LINES: usize = 64;

/// 連鎖を辿る回数の上限。壊れたログで無限ループしないための保険。
const MAX_HOPS: usize = 64;

/// pin した pinned から /clear の連鎖を辿り、いま書き込まれているログの
/// session id を返す。
///
/// * project_dir — 解決済みの Claude プロジェクトディレクトリ。
/// * not_before — このパネルの Claude プロセスを起動した時刻。古いセッションを
///   --resume した直後などに、起動より前から存在する /clear 始まりのログを
///   後続と誤認しないための下限。
/// * claimed — 他の Claude パネルが pin している session id。
///
/// 後続が見つからなければ pinned をそのまま返す。
pub fn resolve_current_session_id(
    project_dir: &Path,
    pinned: &str,
    not_before: SystemTime,
    claimed: &HashSet<String>,
) -> String {
    let mut current = pinned.to_string();
    let mut visited: HashSet<String> = HashSet::new();
    visited.insert(current.clone());

    for _ in 0..MAX_HOPS {
        let Some(current_path) = session_log_in_dir(project_dir, &current) else {
            break;
        };
        // 下限は「前セッションの最終書き込み」。ただし起動時刻より前には
        // 遡らせない (resume 直後で mtime が何日も前のことがある)。
        let lower = last_write(&current_path)
            .and_then(|m| m.checked_sub(MTIME_SLACK))
            .map_or(not_before, |m| m.max(not_before));

        let Some(next) = next_cleared_log(project_dir, lower, &visited, claimed) else {
            break;
        };
        visited.insert(next.clone());
        current = next;
    }
    current
}

/// lower から [MAX_CLEAR_GAP] 以内に始まった /clear 始まりのログのうち、開始が最も
/// 早いものの session id。
fn next_cleared_log(
    project_dir: &Path,
    lower: SystemTime,
    visited: &HashSet<String>,
    claimed: &HashSet<String>,
) -> Option<String> {
    let upper = lower.checked_add(MAX_CLEAR_GAP)?;
    let mut best: Option<(SystemTime, String)> = None;

    for entry in std::fs::read_dir(project_dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if visited.contains(id) || claimed.contains(id) {
            continue;
        }
        let Some(started) = cleared_log_start(&path) else {
            continue;
        };
        if started < lower || started > upper {
            continue;
        }
        if best.as_ref().is_none_or(|(best_ts, _)| started < *best_ts) {
            best = Some((started, id.to_string()));
        }
    }

    best.map(|(_, id)| id)
}

/// 判定は「最初の表示対象ユーザレコードが /clear か」。caveat のような isMeta や、
/// 会話でないレコードは読み飛ばす。通常のセッションは最初が人間のプロンプトなので弾かれる。
fn cleared_log_start(path: &Path) -> Option<SystemTime> {
    use std::io::BufRead;

    let file = std::fs::File::open(path).ok()?;
    let reader = std::io::BufReader::new(file);

    let mut first_ts: Option<SystemTime> = None;

    for line in reader.lines().take(HEAD_LINES) {
        let Ok(line) = line else { break };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(record) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };

        if first_ts.is_none() {
            first_ts = record
                .get("timestamp")
                .and_then(|t| t.as_str())
                .and_then(parse_timestamp);
        }

        if record.get("type").and_then(|t| t.as_str()) != Some("user") {
            continue;
        }
        if record.get("isMeta").and_then(|m| m.as_bool()) == Some(true) {
            continue;
        }

        // 最初の表示対象ユーザレコード。これが /clear でなければ、この
        // ログは別の会話の始まりであってローテーション先ではない。
        let content = record
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .unwrap_or_default();
        if !content.trim_start().starts_with("<command-name>/clear<") {
            return None;
        }
        return first_ts.or_else(|| {
            record
                .get("timestamp")
                .and_then(|t| t.as_str())
                .and_then(parse_timestamp)
        });
    }
    None
}

/// ログへの最終書き込み時刻 (ファイルの mtime)。
fn last_write(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

/// ログ中の RFC3339 タイムスタンプを SystemTime に変換する。
fn parse_timestamp(raw: &str) -> Option<SystemTime> {
    chrono::DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|dt| SystemTime::from(dt.with_timezone(&chrono::Utc)))
}
