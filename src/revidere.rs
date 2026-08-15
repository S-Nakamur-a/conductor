//! revidere の成果物 (`<worktree>/.conductor/review.json`) を読み、読む順を組む。
//!
//! ここは読むだけ。成果物を作る側は [crate::app::revidere]。
//!
//! 型を宣言し直さない
//!
//! Importance も Side も Section も revidere のライブラリ側にあるものを
//! そのまま使う。ホストが写しを持つと、スキーマが動いたときに黙って壊れる。
//! 行から重要度を引く索引も同様で、[revidere::ReadingOrder] に任せる。
//!
//! なぜ diff まで revidere から取るのか
//!
//! conductor は git2 で自前の diff を持っているが、読む順を組むには解析が
//! 見たのと同じ diff が要る。パスの綴り・削除行の前像行番号・rename の扱いが
//! 1 つでもずれると、項目が指す位置が変更一覧から外れて「説明の無い変更行」に化ける。
//! 同じ関数から取れば、その食い違いは起きようがない。

use std::path::Path;

use revidere::{Annotations, ReadingOrder, Scope};

/// 読み込み済みの成果物と、そこから組んだ読む順。
pub struct Review {
    /// 項目・概要・機能への影響・説明もれ検査。行から項目を引く索引を内部に持つ。
    pub annotations: Annotations,
    /// diff を歩いて重要度順に並べたもの。画面に出るのはこちら。
    pub order: ReadingOrder,
    /// 成果物が対象にしていた区間の起点。画面の見出しに出す。終点は作業ツリー
    /// なので、対になる commit id は無い (解析時の HEAD は annotations 側)。
    pub base: String,
}

impl Review {
    /// 全ての変更箇所がちょうど 1 つの項目に属し、行番号の作り話も無いか。
    pub fn is_complete(&self) -> bool {
        self.annotations.coverage().is_complete()
    }

    /// 変更一覧にある変更箇所の総数。
    pub fn total_positions(&self) -> usize {
        self.annotations.coverage().total
    }
}

/// [load] の結果。「無い」と「壊れている」を畳まない。
pub enum LoadOutcome {
    /// 成果物がまだ無い。revidere を走らせていないだけなので正常な状態で、
    /// 呼ぶ側は素の diff を描けばよい。
    Missing,
    /// 読めた。
    Loaded(Box<Review>),
    /// 読めるはずのものが読めなかった。JSON が壊れている、スキーマ版が違う、
    /// あるいは diff が取れない。どれもユーザに見せる価値のある異常。
    Broken(String),
}

/// worktree の成果物を読み、読む順まで組む。
///
/// 「ファイルが無い」をライブラリではなくここで判定するのは revidere の
/// 想定どおり。ホストにとって成果物が無いのは正常な状態なので、ライブラリの
/// 関心から外してある。
pub fn load(worktree: &Path, scope: Scope) -> LoadOutcome {
    let path = revidere::review::artifact_path(worktree, scope);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return LoadOutcome::Missing,
        Err(e) => return LoadOutcome::Broken(format!("{}: {e}", path.display())),
    };
    let annotations = match Annotations::from_json(&text) {
        Ok(a) => a,
        Err(e) => return LoadOutcome::Broken(format!("{}: {e}", path.display())),
    };
    // 前回からの差分は 1 ラウンド 1 枚。起点が今の前回と違えば、それは前の回の
    // 成果物で、この回についてはまだ解析していないのと同じ。壊れてはいないので
    // Broken ではなく Missing に寄せる。
    if scope == Scope::SincePrevious
        && Some(annotations.base()) != previous_head(worktree).as_deref()
    {
        log::info!(
            "revidere: {} is from an earlier round (base {})",
            path.display(),
            annotations.base()
        );
        return LoadOutcome::Missing;
    }

    // 成果物が見ていたのと同じ区間の diff を取る。起点は成果物に書いてある
    // コミット ID をそのまま使う — ここで base を推定し直すと、解析のあとに
    // ベースが動いただけで項目の指す位置が全部ずれる。
    let raw = match revidere::git::diff(worktree, annotations.base()) {
        Ok(d) => d,
        Err(e) => return LoadOutcome::Broken(format!("diff を取れない: {}", e.0)),
    };
    let diff = revidere::diff::parse(&raw);
    let order = ReadingOrder::build(&diff, &annotations);

    LoadOutcome::Loaded(Box::new(Review {
        base: annotations.base().to_string(),
        annotations,
        order,
    }))
}

/// いまのブランチ全体のレビューが「前回」と見ているコミット。
///
/// 差分を解析するときの起点であり、その成果物が今の回のものかを見分ける鍵
/// でもある。写しを 2 か所に持つと、片方だけ古くなったときに黙って別の区間を
/// 見ることになるので、持ち主はブランチ全体の成果物 1 つだけ。
pub fn previous_head(worktree: &Path) -> Option<String> {
    let path = revidere::review::artifact_path(worktree, Scope::Base);
    let text = std::fs::read_to_string(path).ok()?;
    let annotations = Annotations::from_json(&text).ok()?;
    Some(annotations.since_previous()?.previous_head.clone())
}

/// 画面とステータスに出す区間の呼び名。1 か所で持つ。
pub fn scope_label(scope: Scope) -> &'static str {
    match scope {
        Scope::Base => "ブランチ全体",
        Scope::SincePrevious => "前回のレビューから",
    }
}

/// 重要度の色。revidere の目安をテーマの世界へ写す。
///
/// 意味は [revidere::Importance] の側にあり、こちらが決めるのは見え方だけ。
pub fn importance_color(importance: revidere::Importance) -> ratatui::style::Color {
    let (r, g, b) = importance.recommended_rgb();
    ratatui::style::Color::Rgb(r, g, b)
}

/// worktree の解析がいまどの状態か。worktree ストリップに常時出す。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactState {
    /// 解析していない (成果物が無い)。何も出さない。
    None,
    /// 解析が走っている。
    Running,
    /// 成果物があり、いまの HEAD より後に作られている。
    Fresh,
    /// 成果物はあるが、その後にコミットが載っている。
    Stale,
}

impl ArtifactState {
    /// ストリップに出す 1 文字。空なら出さない。
    ///
    /// 色だけで区別すると、色が近いテーマや色覚の違いで読めなくなるので、
    /// 形でも分かるようにしてある。実行中はここを呼ばず、呼び出し側が
    /// 回るスピナーに差し替える — 動いているものは動いて見えるのが早い。
    pub fn marker(self) -> &'static str {
        match self {
            Self::None | Self::Running => "",
            Self::Fresh => "\u{2713}", // ✓
            Self::Stale => "!",
        }
    }
}

/// worktree の解析の状態を返す。
///
/// 古いかどうかは「成果物のファイルが HEAD コミットより前に書かれたか」で
/// 見る。解析時の commit id はどこにも書き残していないので突き合わせられない
/// が、commit / amend / rebase / merge はどれも新しい committer 時刻を刻むので、
/// 前へ進む操作はこれで全部捕まる。捕まらないのは古いコミットへ戻したとき
/// (checkout / reset --hard) だけ。
///
/// 時刻で見ることには利点もあって、端末から直接 revidere を走らせた成果物も、
/// conductor を再起動したあとも、同じように判定できる。conductor が自分で
/// 覚えている必要がない。
pub fn artifact_state(worktree: &Path, head_time: Option<i64>, running: bool) -> ArtifactState {
    if running {
        return ArtifactState::Running;
    }
    let path = revidere::review::artifact_path(worktree, Scope::Base);
    let Ok(modified) = std::fs::metadata(&path).and_then(|m| m.modified()) else {
        return ArtifactState::None;
    };
    let Some(head_time) = head_time else {
        // コミットがまだ無いブランチ。比べる相手が無いので古くはない。
        return ArtifactState::Fresh;
    };
    let written = modified
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    if written < head_time {
        ArtifactState::Stale
    } else {
        ArtifactState::Fresh
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 成果物が無いのは正常。ここを Broken に畳むと、revidere を走らせて
    /// いないだけの worktree でエラーが出続けることになる。
    #[test]
    fn a_missing_artifact_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            load(dir.path(), Scope::Base),
            LoadOutcome::Missing
        ));
    }

    /// 読めたテキストが成果物として通らないのは異常で、黙って Missing に
    /// してはいけない。「走らせたのに何も出ない」の原因が見えなくなる。
    #[test]
    fn a_broken_artifact_reports_why() {
        let dir = tempfile::tempdir().unwrap();
        let path = revidere::review::artifact_path(dir.path(), Scope::Base);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{ not json").unwrap();
        match load(dir.path(), Scope::Base) {
            LoadOutcome::Broken(msg) => assert!(msg.contains("JSON"), "got: {msg}"),
            _ => panic!("壊れた JSON は Broken になるはず"),
        }
    }

    /// 成果物 1 枚ぶんの JSON。区間の起点と、前回の起点だけを差し替える。
    fn artifact(base: &str, previous: Option<&str>) -> String {
        let since = previous
            .map(|p| {
                format!(
                    r#","since_previous":{{"previous_head":"{p}","head":"h","files":[],"history_rewritten":false}}"#
                )
            })
            .unwrap_or_default();
        format!(
            r#"{{"schema":2,"base":"{base}","head":"h","overview":{{"problem":"","change":"","mechanism":"","placement":"","scope":""}},"sections":[],"impacts":[],"coverage":{{"total":0,"classified":0,"unclassified":[],"conflicts":[],"unknown":[]}}{since}}}"#
        )
    }

    /// 前の回の成果物が残っていても、それを今の回として出さない。起点が違えば
    /// 区間が違うので、直しを読んでいるつもりで 1 つ前のラウンドを読むことになる。
    #[test]
    fn a_since_previous_artifact_from_an_earlier_round_counts_as_missing() {
        let dir = tempfile::tempdir().unwrap();
        let base_path = revidere::review::artifact_path(dir.path(), Scope::Base);
        std::fs::create_dir_all(base_path.parent().unwrap()).unwrap();
        std::fs::write(&base_path, artifact("b0", Some("p2"))).unwrap();
        std::fs::write(
            revidere::review::artifact_path(dir.path(), Scope::SincePrevious),
            // 起点が今の前回 (p2) ではなく、1 つ前の回のもの。
            artifact("p1", None),
        )
        .unwrap();

        assert!(matches!(
            load(dir.path(), Scope::SincePrevious),
            LoadOutcome::Missing
        ));
    }

    /// 起点が今の前回と一致していれば、それは今の回のもの。Missing に
    /// 畳んでしまうと、解析済みでも毎回作り直させることになる。
    #[test]
    fn a_since_previous_artifact_for_this_round_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let base_path = revidere::review::artifact_path(dir.path(), Scope::Base);
        std::fs::create_dir_all(base_path.parent().unwrap()).unwrap();
        std::fs::write(&base_path, artifact("b0", Some("p2"))).unwrap();
        std::fs::write(
            revidere::review::artifact_path(dir.path(), Scope::SincePrevious),
            artifact("p2", None),
        )
        .unwrap();

        // git の無い一時ディレクトリなので diff は取れず Broken まで進む。
        // ここで見たいのは「起点が合っていれば捨てられない」ことだけ。
        assert!(matches!(
            load(dir.path(), Scope::SincePrevious),
            LoadOutcome::Broken(_)
        ));
    }

    /// 成果物を書いてから積んだコミットは、成果物を古くすること。
    #[test]
    fn a_commit_made_after_the_artifact_makes_it_stale() {
        let dir = tempfile::tempdir().unwrap();
        let path = revidere::review::artifact_path(dir.path(), Scope::Base);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{}").unwrap();

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        // HEAD のほうが後 = 成果物のあとにコミットした。
        assert_eq!(
            artifact_state(dir.path(), Some(now + 60), false),
            ArtifactState::Stale
        );
        // HEAD のほうが前 = 成果物はいまの HEAD を見て作られている。
        assert_eq!(
            artifact_state(dir.path(), Some(now - 60), false),
            ArtifactState::Fresh
        );
        // 解析中はファイルの新旧に関わらず走っていることが優先。
        assert!(artifact_state(dir.path(), Some(now + 60), true) == ArtifactState::Running);
    }

    /// 成果物が無ければ何も出さないこと。走らせていないだけの worktree に
    /// 印が付くと、印そのものが意味を失う。
    #[test]
    fn no_artifact_shows_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let state = artifact_state(dir.path(), Some(0), false);
        assert_eq!(state, ArtifactState::None);
        assert!(state.marker().is_empty());
    }
}
