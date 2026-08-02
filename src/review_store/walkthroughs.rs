//! AI walkthrough 生成のライフサイクル: ブランチの walkthrough の開始・保存・
//! 失敗記録・取得（walkthroughs テーブルと walkthrough_steps テーブル）。

use anyhow::Result;
use rusqlite::params;
use uuid::Uuid;

use crate::walkthrough::{
    NewWalkthroughStep, Walkthrough, WalkthroughStatus, WalkthroughStep, WalkthroughStepKind,
};

use super::{Author, ReviewStore};

impl ReviewStore {
    /// ブランチの walkthrough 生成を開始（または再開始）する。既存の
    /// walkthrough があれば削除し（生成履歴は保持しない。v6 マイグレーションの
    /// 注記を参照）、生成中の新しい行を挿入する。これにより呼び出し側は
    /// 完了をポーリングしたり、生成が止まっていることを検知したりできる id
    /// を得る。生成対象のブランチ先端（head_commit、HEAD コミットの OID）も
    /// 記録し、後で同一コミットに対する再生成をスキップできるようにする。
    /// 先端が不明な場合は None を渡す。
    pub fn begin_walkthrough(
        &self,
        branch: &str,
        head_commit: Option<&str>,
    ) -> Result<Walkthrough> {
        self.conn.execute(
            "DELETE FROM walkthroughs WHERE branch = ?1",
            params![branch],
        )?;
        let id = Uuid::new_v4().to_string();
        self.conn.execute(
            "INSERT INTO walkthroughs (id, branch, status, head_commit)
             VALUES (?1, ?2, 'generating', ?3)",
            params![id, branch, head_commit],
        )?;
        self.walkthrough_row_by_id(&id)
    }

    /// ブランチの完成した walkthrough を保存する。walkthrough 行を upsert し、
    /// そのステップを1つのトランザクション内で置き換える。これは本番の
    /// 書き込み経路であり、conductor mcp-serve バイナリの save_walkthrough
    /// ツールがこのメソッドをプロセス内で直接呼び出す（同期を取るための
    /// 別実装は存在しない）。begin_walkthrough はもはや前提条件ではなく、
    /// 事前の walkthrough がないブランチに対して呼べば新規作成され、
    /// 再度呼べば以前のものを完全に置き換える（v6 マイグレーションで
    /// 説明した「生成履歴は保持しない」というモデルどおり）。
    ///
    /// walkthrough の id を返す。既存行に当たった upsert の場合、これは
    /// ここで生成した id ではなくその既存行の id であり、呼び出し側は
    /// 実際に有効な id を報告できる。
    ///
    /// summary は意図的に2箇所に書き込まれる。walkthrough 行自体（往復の
    /// 忠実性のため）と、SUMMARY 疑似ファイルの裏にある change_summary の
    /// 両方である。change_summary テーブルに書き込むのは walkthrough の
    /// 生成だけなので、この2つは同時に反映されなければならない。だから
    /// 同じトランザクション内で行い、保存に失敗した場合に、実際には
    /// 保存されなかった walkthrough を説明するサマリだけが残る事態を防ぐ。
    pub fn save_walkthrough(
        &self,
        branch: &str,
        title: &str,
        summary: &str,
        steps: &[NewWalkthroughStep],
    ) -> Result<String> {
        let candidate_id = Uuid::new_v4().to_string();

        // 素の BEGIN ではなく BEGIN IMMEDIATE を使い、先に書き込みロックを
        // 取得する。WAL の下では、書き込みより先に読み込みを行う deferred な
        // トランザクションは、他の書き込みが競合した際に SQLITE_BUSY ではなく
        // SQLITE_BUSY_SNAPSHOT を受け取ることがある。busy_timeout のリトライ
        // 処理は後者にしか反応しないため、ここで deferred なトランザクションを
        // 使うと待たずに失敗しかねない。
        self.conn.execute_batch("BEGIN IMMEDIATE;")?;
        let result = (|| -> Result<String> {
            self.conn.execute(
                "INSERT INTO walkthroughs (id, branch, title, summary, status, error, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, 'ready', NULL,
                         COALESCE((SELECT created_at FROM walkthroughs WHERE branch = ?5), datetime('now')),
                         datetime('now'))
                 ON CONFLICT(branch) DO UPDATE SET
                     title = excluded.title, summary = excluded.summary,
                     status = 'ready', error = NULL, updated_at = datetime('now')",
                params![candidate_id, branch, title, summary, branch],
            )?;
            self.save_change_summary(branch, summary, Author::Claude)?;

            // 競合時、INSERT は candidate_id ではなく既存行の id を保持する
            // ため、今生成した id を前提にせず、実際に有効な id を読み直す。
            let walkthrough_id: String = self.conn.query_row(
                "SELECT id FROM walkthroughs WHERE branch = ?1",
                params![branch],
                |row| row.get(0),
            )?;

            self.conn.execute(
                "DELETE FROM walkthrough_steps WHERE walkthrough_id = ?1",
                params![walkthrough_id],
            )?;
            // seq は呼び出し側が渡した値ではなく、スライスの並び順から決める。
            // MCP ツールはステップごとの seq を受け付けるため、種類ごとに
            // 番号を振るモデル（intent 0,1 / core 0,1,2 / …）だと、それを
            // そのまま使った場合ツアー全体の順序が入り乱れてしまう。しかも
            // 見た目はきれいにレンダリングされ、成功したと報告され、物語の
            // 順序が失われたことを示すものは何もない。ここで導出することで
            // walkthrough ごとに seq が密で一意にもなり、get_walkthrough の
            // ORDER BY seq にタイブレークが不要になる。
            for (seq, step) in steps.iter().enumerate() {
                let step_id = Uuid::new_v4().to_string();
                self.conn.execute(
                    "INSERT INTO walkthrough_steps
                        (id, walkthrough_id, seq, file_path, line_start, line_end, kind, title, body)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        step_id,
                        walkthrough_id,
                        seq as i64,
                        step.file_path,
                        step.line_start,
                        step.line_end,
                        step.kind.as_str(),
                        step.title,
                        step.body,
                    ],
                )?;
            }
            Ok(walkthrough_id)
        })();

        match result {
            // COMMIT が失敗してもトランザクションは開いたままになるため、
            // ステートメントの失敗と同じくロールバックが必要になる。そうしない
            // と、この接続でのその後の書き込みがすべて取り残されたトランザク
            // ションに巻き込まれ、成功したと報告されつつも、プロセス終了時に
            // 破棄されてしまう。
            Ok(walkthrough_id) => match self.conn.execute_batch("COMMIT;") {
                Ok(()) => Ok(walkthrough_id),
                Err(e) => {
                    let _ = self.conn.execute_batch("ROLLBACK;");
                    Err(e.into())
                }
            },
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK;");
                Err(e)
            }
        }
    }

    /// ブランチの walkthrough を失敗としてマークし、理由を記録する。事前に
    /// begin_walkthrough がその行を作成していることが前提。
    pub fn fail_walkthrough(&self, branch: &str, error: &str) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE walkthroughs
             SET status = 'failed', error = ?1, updated_at = datetime('now')
             WHERE branch = ?2",
            params![error, branch],
        )?;
        if changed == 0 {
            anyhow::bail!("no walkthrough row for branch {branch} — call begin_walkthrough first");
        }
        Ok(())
    }

    /// ブランチの walkthrough ヘッダーとそのステップ（seq 順）を取得する。
    /// walkthrough が開始されていなければ None を返す。
    pub fn get_walkthrough(
        &self,
        branch: &str,
    ) -> Result<Option<(Walkthrough, Vec<WalkthroughStep>)>> {
        let walkthrough = match self.conn.query_row(
            "SELECT id, branch, title, summary, status, error, created_at, updated_at, head_commit
             FROM walkthroughs WHERE branch = ?1",
            params![branch],
            row_to_walkthrough,
        ) {
            Ok(w) => w,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
            Err(e) => return Err(e.into()),
        };

        // seq は書き込み時にスライスの並び順から割り当てられるため、
        // walkthrough 内で密かつ一意になる。ORDER BY seq だけで十分な全順序
        // であり、タイブレークは不要（id でのタイブレークはむしろ有害。
        // ステップの id はランダムな UUID なので、それでタイブレークすると
        // 保存された順序ではなくランダムな順序になってしまう）。
        let mut stmt = self.conn.prepare(
            "SELECT id, walkthrough_id, seq, file_path, line_start, line_end, kind, title, body
             FROM walkthrough_steps
             WHERE walkthrough_id = ?1
             ORDER BY seq",
        )?;
        let rows = stmt.query_map(params![walkthrough.id], row_to_walkthrough_step)?;
        let mut steps = Vec::new();
        for row in rows {
            steps.push(row?);
        }
        Ok(Some((walkthrough, steps)))
    }

    /// id で walkthrough 行を取得する（begin_walkthrough が挿入した直後に、
    /// created_at のようなサーバ側のデフォルト値を読み戻すために使う）。
    fn walkthrough_row_by_id(&self, id: &str) -> Result<Walkthrough> {
        self.conn
            .query_row(
                "SELECT id, branch, title, summary, status, error, created_at, updated_at, head_commit
                 FROM walkthroughs WHERE id = ?1",
                params![id],
                row_to_walkthrough,
            )
            .map_err(Into::into)
    }
}

fn row_to_walkthrough(row: &rusqlite::Row<'_>) -> rusqlite::Result<Walkthrough> {
    let status_str: String = row.get(4)?;
    let status = WalkthroughStatus::from_str(&status_str).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            4,
            rusqlite::types::Type::Text,
            format!("unknown WalkthroughStatus: {status_str}").into(),
        )
    })?;

    Ok(Walkthrough {
        id: row.get(0)?,
        branch: row.get(1)?,
        title: row.get(2)?,
        summary: row.get(3)?,
        status,
        error: row.get(5)?,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
        head_commit: row.get(8)?,
    })
}

fn row_to_walkthrough_step(row: &rusqlite::Row<'_>) -> rusqlite::Result<WalkthroughStep> {
    let kind_str: String = row.get(6)?;
    let kind = WalkthroughStepKind::from_str(&kind_str).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            6,
            rusqlite::types::Type::Text,
            format!("unknown WalkthroughStepKind: {kind_str}").into(),
        )
    })?;

    // 書き込み時だけでなく読み出し時にも正規化する。save_walkthrough が
    // 正規化するようになる前に書かれた行（./src/a.rs として保存された
    // ステップなど）は、そうしないと FileDiff::path に決して一致せず、
    // ステップへジャンプできなくなってしまう。マイグレーションではなくここで
    // 行うことで、同じ修正が同じデータベースに対して古い conductor が
    // 書いた行もカバーする。
    let file_path: String = row.get(3)?;
    let file_path = crate::repo_path::normalize(&file_path);

    Ok(WalkthroughStep {
        id: row.get(0)?,
        walkthrough_id: row.get(1)?,
        seq: row.get(2)?,
        file_path,
        line_start: row.get(4)?,
        line_end: row.get(5)?,
        kind,
        title: row.get(7)?,
        body: row.get(8)?,
    })
}

#[cfg(test)]
mod tests {
    use super::super::test_support::test_store;
    use super::*;

    #[test]
    fn walkthrough_lifecycle() {
        let store = test_store();

        // まだ walkthrough はない。
        assert!(store.get_walkthrough("feat/x").unwrap().is_none());

        let started = store.begin_walkthrough("feat/x", Some("abc1234")).unwrap();
        assert_eq!(started.branch, "feat/x");
        assert_eq!(started.status, WalkthroughStatus::Generating);
        // ブランチ先端を記録しておき、同一コミットでの再生成をスキップできるようにする。
        assert_eq!(started.head_commit.as_deref(), Some("abc1234"));

        let (walkthrough, steps) = store.get_walkthrough("feat/x").unwrap().unwrap();
        assert_eq!(walkthrough.id, started.id);
        assert_eq!(walkthrough.status, WalkthroughStatus::Generating);
        assert_eq!(walkthrough.head_commit.as_deref(), Some("abc1234"));
        assert!(steps.is_empty());

        let new_steps = vec![
            NewWalkthroughStep {
                file_path: "src/main.rs".to_string(),
                line_start: Some(10),
                line_end: Some(20),
                kind: WalkthroughStepKind::Intent,
                title: "Why this change exists".to_string(),
                body: "Fixes a startup crash.".to_string(),
            },
            NewWalkthroughStep {
                file_path: "src/lib.rs".to_string(),
                line_start: None,
                line_end: None,
                kind: WalkthroughStepKind::Core,
                title: "Core fix".to_string(),
                body: "Guards against the null case.".to_string(),
            },
        ];
        store
            .save_walkthrough(
                "feat/x",
                "Fix startup crash",
                "A short summary.",
                &new_steps,
            )
            .unwrap();

        let (walkthrough, steps) = store.get_walkthrough("feat/x").unwrap().unwrap();
        assert_eq!(walkthrough.status, WalkthroughStatus::Ready);
        assert_eq!(walkthrough.title.as_deref(), Some("Fix startup crash"));
        assert_eq!(walkthrough.summary.as_deref(), Some("A short summary."));
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].seq, 0);
        assert_eq!(steps[0].file_path, "src/main.rs");
        assert_eq!(steps[0].kind, WalkthroughStepKind::Intent);
        assert_eq!(steps[1].seq, 1);
        assert_eq!(steps[1].kind, WalkthroughStepKind::Core);

        // 再生成は行全体を置き換える（履歴は保持しない）。先端を渡さなければ
        // head_commit は null のままになる。
        let restarted = store.begin_walkthrough("feat/x", None).unwrap();
        assert_ne!(restarted.id, started.id);
        assert_eq!(restarted.head_commit, None);
        let (walkthrough, steps) = store.get_walkthrough("feat/x").unwrap().unwrap();
        assert_eq!(walkthrough.status, WalkthroughStatus::Generating);
        assert!(steps.is_empty());

        store
            .fail_walkthrough("feat/x", "Claude Code exited early")
            .unwrap();
        let (walkthrough, _) = store.get_walkthrough("feat/x").unwrap().unwrap();
        assert_eq!(walkthrough.status, WalkthroughStatus::Failed);
        assert_eq!(
            walkthrough.error.as_deref(),
            Some("Claude Code exited early")
        );
    }

    /// 保存時にパスを正規化するようになる前に書かれた行も解決できなければ
    /// ならない。このステップは古い save_walkthrough が使っていた生の SQL で
    /// 挿入しているので、現在の書き込み経路が生成し得る値ではなく、正真正銘の
    /// レガシー行になっている。
    #[test]
    fn legacy_step_paths_are_normalised_on_read() {
        let store = test_store();
        store
            .save_walkthrough("feat/x", "title", "summary", &[])
            .unwrap();
        let (walkthrough, _) = store.get_walkthrough("feat/x").unwrap().unwrap();
        for (seq, stored) in ["./src/a.rs", "src//b.rs", "src/c.rs/"].iter().enumerate() {
            store
                .conn
                .execute(
                    "INSERT INTO walkthrough_steps
                        (id, walkthrough_id, seq, file_path, kind, title, body)
                     VALUES (?1, ?2, ?3, ?4, 'core', 'legacy', 'body')",
                    params![
                        Uuid::new_v4().to_string(),
                        walkthrough.id,
                        seq as i64,
                        stored
                    ],
                )
                .unwrap();
        }

        let (_, steps) = store.get_walkthrough("feat/x").unwrap().unwrap();
        assert_eq!(
            steps
                .iter()
                .map(|s| s.file_path.as_str())
                .collect::<Vec<_>>(),
            vec!["src/a.rs", "src/b.rs", "src/c.rs"]
        );
    }

    #[test]
    fn fail_walkthrough_without_begin_is_an_error() {
        let store = test_store();
        assert!(store.fail_walkthrough("feat/x", "boom").is_err());
    }

    /// save_walkthrough はもう begin_walkthrough を必要としない。walkthrough
    /// 行自体を upsert するので、単独でも有効なエントリポイントになる
    /// （mcp-serve ツールが直接呼ぶのはこれ）。
    #[test]
    fn save_walkthrough_without_begin_upserts() {
        let store = test_store();
        assert!(store.get_walkthrough("feat/x").unwrap().is_none());

        store
            .save_walkthrough("feat/x", "title", "summary", &[])
            .unwrap();

        let (walkthrough, _) = store.get_walkthrough("feat/x").unwrap().unwrap();
        assert_eq!(walkthrough.status, WalkthroughStatus::Ready);
        assert_eq!(walkthrough.title.as_deref(), Some("title"));
    }

    /// 2回保存すると、ステップは追記されるのではなく完全に置き換わらなければ
    /// ならない（トランザクション内の DELETE + 再 INSERT で、
    /// walkthrough_steps.walkthrough_id の CASCADE に支えられている）。
    #[test]
    fn save_walkthrough_replaces_previous_steps() {
        let store = test_store();

        store
            .save_walkthrough(
                "feat/x",
                "title",
                "summary",
                &[NewWalkthroughStep {
                    file_path: "src/old.rs".to_string(),
                    line_start: None,
                    line_end: None,
                    kind: WalkthroughStepKind::Intent,
                    title: "First pass".to_string(),
                    body: "Old body.".to_string(),
                }],
            )
            .unwrap();

        store
            .save_walkthrough(
                "feat/x",
                "title",
                "summary",
                &[NewWalkthroughStep {
                    file_path: "src/new.rs".to_string(),
                    line_start: None,
                    line_end: None,
                    kind: WalkthroughStepKind::Core,
                    title: "Second pass".to_string(),
                    body: "New body.".to_string(),
                }],
            )
            .unwrap();

        let (_, steps) = store.get_walkthrough("feat/x").unwrap().unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].file_path, "src/new.rs");
        assert_eq!(steps[0].kind, WalkthroughStepKind::Core);
        assert_eq!(steps[0].body, "New body.");
    }

    /// walkthrough の summary はブランチの change summary でもある —
    /// SUMMARY 疑似ファイルの内容そのものである。walkthrough の生成だけが
    /// これを書き込むので、この結びつきが壊れると SUMMARY ペインは何も言わず
    /// 永遠に空のままになってしまう。
    #[test]
    fn save_walkthrough_also_writes_the_change_summary() {
        let store = test_store();
        store.begin_walkthrough("feat/x", None).unwrap();
        assert_eq!(store.get_change_summary("feat/x").unwrap(), None);

        store
            .save_walkthrough("feat/x", "Fix startup crash", "何をなぜ変えたか。", &[])
            .unwrap();

        assert_eq!(
            store.get_change_summary("feat/x").unwrap().as_deref(),
            Some("何をなぜ変えたか。")
        );

        // 再生成すると置き換わるので、ペインは古いものを溜め込むのではなく
        // 常に最新の概要を表示する。
        store.begin_walkthrough("feat/x", None).unwrap();
        store
            .save_walkthrough("feat/x", "Fix startup crash", "更新後の概要。", &[])
            .unwrap();
        assert_eq!(
            store.get_change_summary("feat/x").unwrap().as_deref(),
            Some("更新後の概要。")
        );
    }

    /// save_walkthrough の doc コメントにあるトランザクション安全性の主張を
    /// 証明する: ステップの挿入が失敗した場合、同じトランザクション内で
    /// 一緒に書き込まれた walkthrough 行と change summary もロールバック
    /// されなければならず、実際には保存されなかった walkthrough を説明する
    /// summary だけが残ってはならない。失敗は不正な引数ではなくトリガーで
    /// 注入している。save_walkthrough 自身が拒否するような引数の形は、
    /// どんな書き込みが起きるより前にすでに弾かれてしまうため。
    #[test]
    fn failed_step_insert_leaves_no_change_summary() {
        let store = test_store();
        store
            .conn
            .execute_batch(
                "CREATE TRIGGER boom BEFORE INSERT ON walkthrough_steps
                 BEGIN SELECT RAISE(ABORT, 'boom'); END",
            )
            .unwrap();

        let steps = vec![NewWalkthroughStep {
            file_path: "src/main.rs".to_string(),
            line_start: None,
            line_end: None,
            kind: WalkthroughStepKind::Intent,
            title: "won't be saved".to_string(),
            body: "the trigger aborts before this lands".to_string(),
        }];
        assert!(
            store
                .save_walkthrough("feat/x", "t", "summary", &steps)
                .is_err()
        );

        // ROLLBACK は、実際に失敗したステップの挿入だけでなく、walkthrough の
        // upsert と change summary の書き込みの両方を取り消す。
        assert_eq!(store.get_change_summary("feat/x").unwrap(), None);
        assert!(store.get_walkthrough("feat/x").unwrap().is_none());
    }

    /// スライスの順序が walkthrough の順序であり、seq はそこから導出される
    /// — だからステップは常に渡された順序どおりに、密な 0..n として返って
    /// くる。これにより、種類ごとにステップ番号を振る呼び出し側がツアーを
    /// 気づかぬうちに入り乱れさせてしまうのを防いでいる。
    #[test]
    fn save_walkthrough_numbers_steps_by_slice_order() {
        let store = test_store();
        let steps = vec![
            NewWalkthroughStep {
                file_path: "src/a.rs".to_string(),
                line_start: None,
                line_end: None,
                kind: WalkthroughStepKind::Intent,
                title: "a".to_string(),
                body: "a".to_string(),
            },
            NewWalkthroughStep {
                file_path: "src/b.rs".to_string(),
                line_start: None,
                line_end: None,
                kind: WalkthroughStepKind::Core,
                title: "b".to_string(),
                body: "b".to_string(),
            },
            NewWalkthroughStep {
                file_path: "src/c.rs".to_string(),
                line_start: None,
                line_end: None,
                kind: WalkthroughStepKind::Ripple,
                title: "c".to_string(),
                body: "c".to_string(),
            },
        ];
        store
            .save_walkthrough("feat/x", "title", "summary", &steps)
            .unwrap();

        let (_, loaded) = store.get_walkthrough("feat/x").unwrap().unwrap();
        assert_eq!(
            loaded.iter().map(|s| s.seq).collect::<Vec<_>>(),
            vec![0, 1, 2],
            "seq must be dense and follow the slice order"
        );
        assert_eq!(
            loaded
                .iter()
                .map(|s| s.file_path.as_str())
                .collect::<Vec<_>>(),
            vec!["src/a.rs", "src/b.rs", "src/c.rs"],
            "steps must come back in the order they were supplied"
        );
    }

    /// WalkthroughStepKind::as_str() とスキーマ側の
    /// CHECK (kind IN ('intent','core','ripple','test')) は別々に書かれた
    /// 2つの文字列リストであり、両者がずれた場合、CHECK に欠けている
    /// 種類だけが失敗する — しかもコンパイル時ではなく保存時にである。
    /// これは4種類すべてを試すことで、そのようなずれを即座に検出する。
    #[test]
    fn save_and_load_round_trips_every_step_kind() {
        let store = test_store();
        let kinds = [
            WalkthroughStepKind::Intent,
            WalkthroughStepKind::Core,
            WalkthroughStepKind::Ripple,
            WalkthroughStepKind::Test,
        ];
        let steps: Vec<NewWalkthroughStep> = kinds
            .iter()
            .enumerate()
            .map(|(i, kind)| NewWalkthroughStep {
                file_path: format!("src/{i}.rs"),
                line_start: None,
                line_end: None,
                kind: *kind,
                title: format!("step {i}"),
                body: format!("body {i}"),
            })
            .collect();
        store
            .save_walkthrough("feat/x", "title", "summary", &steps)
            .unwrap();

        let (_, loaded) = store.get_walkthrough("feat/x").unwrap().unwrap();
        assert_eq!(loaded.len(), kinds.len());
        for (step, kind) in loaded.iter().zip(kinds.iter()) {
            assert_eq!(step.kind, *kind);
            // 行からすでに deserialize された enum 値だけでなく、文字列形式
            // 経由の往復もテストする。
            assert_eq!(WalkthroughStepKind::from_str(kind.as_str()), Some(*kind));
        }
    }
}
