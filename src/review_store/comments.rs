//! レビューコメント（reviews テーブル）の CRUD とクエリ。

use anyhow::Result;
use rusqlite::params;
use uuid::Uuid;

use super::ReviewStore;
use super::model::{Author, CommentKind, CommentStatus, ReviewComment};

/// [ReviewStore::resolve_id_prefix] がマッチさせる最短の id プレフィックス長。
///
/// コメント id は Claude には先頭8文字として見えているので、8 は MCP ツールが
/// 公表している長さであり、かつ呼び出し側が実際に目にしている長さでもある。
pub const MIN_ID_PREFIX_LEN: usize = 8;

impl ReviewStore {
    /// 新しいレビューコメントを挿入して返す。
    #[allow(clippy::too_many_arguments)]
    pub fn add_review(
        &self,
        worktree: &str,
        file_path: &str,
        line_start: u32,
        line_end: Option<u32>,
        kind: CommentKind,
        body: &str,
        commit_ref: &str,
        author: Author,
        branch: Option<&str>,
    ) -> Result<ReviewComment> {
        let id = Uuid::new_v4().to_string();

        self.conn.execute(
            "INSERT INTO reviews (id, worktree, file_path, line_start, line_end, kind, body, commit_ref, author, branch)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                id,
                worktree,
                file_path,
                line_start as i64,
                line_end.map(|n| n as i64),
                kind.as_str(),
                body,
                commit_ref,
                author.as_str(),
                branch,
            ],
        )?;

        // サーバ側のデフォルト値（created_at, updated_at）を得るために読み直す。
        self.get_review(&id)
    }

    /// id を指定してレビュー1件を取得する。
    pub fn get_review(&self, id: &str) -> Result<ReviewComment> {
        self.conn
            .query_row(
                "SELECT id, worktree, file_path, line_start, line_end, kind, body, status,
                        author, branch, created_at
                 FROM reviews WHERE id = ?1",
                params![id],
                row_to_review,
            )
            .map_err(Into::into)
    }

    /// レビューコメントの本文テキストを編集する。
    pub fn update_review_body(&self, id: &str, body: &str) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE reviews SET body = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![body, id],
        )?;
        if changed == 0 {
            anyhow::bail!("review not found: {id}");
        }
        Ok(())
    }

    /// id を指定してレビューコメントを1件削除する。
    pub fn delete_review(&self, id: &str) -> Result<()> {
        let changed = self
            .conn
            .execute("DELETE FROM reviews WHERE id = ?1", params![id])?;
        if changed == 0 {
            anyhow::bail!("review not found: {id}");
        }
        Ok(())
    }

    /// レビューコメントの状態を更新する。
    pub fn update_review_status(&self, id: &str, status: CommentStatus) -> Result<()> {
        let changed = self.conn.execute(
            "UPDATE reviews SET status = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![status.as_str(), id],
        )?;
        if changed == 0 {
            anyhow::bail!("review not found: {id}");
        }
        Ok(())
    }

    /// 指定した worktree の全レビューを、ファイル→行の順で返す。
    pub fn reviews_for_worktree(&self, worktree: &str) -> Result<Vec<ReviewComment>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, worktree, file_path, line_start, line_end, kind, body, status,
                    author, branch, created_at
             FROM reviews
             WHERE worktree = ?1
             ORDER BY file_path, line_start",
        )?;
        collect_reviews(&mut stmt, params![worktree])
    }

    /// 指定したレビューコメント群を GitHub に投稿済みとしてマークし、全件に
    /// 同じタイムスタンプを刻む（1回の投稿バッチ＝1つの時刻）。一度設定されると
    /// unpublished_reviews はそれらを返さなくなるので、投稿をリトライしても
    /// 同じコメントを二重投稿することはない。
    pub fn mark_published(&self, comment_ids: &[String], timestamp: &str) -> Result<()> {
        self.conn.execute_batch("BEGIN;")?;
        let result = (|| -> Result<()> {
            for id in comment_ids {
                self.conn.execute(
                    "UPDATE reviews SET published_at = ?1 WHERE id = ?2",
                    params![timestamp, id],
                )?;
            }
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.conn.execute_batch("COMMIT;")?;
                Ok(())
            }
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK;");
                Err(e)
            }
        }
    }

    /// ブランチのレビューコメントのうち、まだ GitHub に投稿されていないもの
    /// （published_at IS NULL）を返す。
    pub fn unpublished_reviews(&self, branch: &str) -> Result<Vec<ReviewComment>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, worktree, file_path, line_start, line_end, kind, body, status,
                    author, branch, created_at
             FROM reviews
             WHERE branch = ?1 AND published_at IS NULL
             ORDER BY file_path, line_start",
        )?;
        collect_reviews(&mut stmt, params![branch])
    }

    /// コメントの完全な id、またはその一意なプレフィックスを、完全な id に解決する。
    ///
    /// [MIN_ID_PREFIX_LEN] より短いものは拒否する。これはツールが公表している
    /// 長さであり、かつコメント id が実際に表示される際の長さでもあるため、
    /// それより短いものは正当な省略ではなく単なる入力ミスとみなす。
    ///
    /// prefix は16進数の文字と - のみ（UUID を構成する文字集合）でなければ
    /// ならず、そうでなければデータベースに触れずに Ok(None) を返す。未検証の
    /// プレフィックスからそのまま LIKE パターンを組み立てると、%/_ が SQL の
    /// ワイルドカードとして働いてしまう（例えば prefix = "%" が任意のコメントに
    /// マッチする）ため、この形を事前に弾いておく。この制限によって正当な
    /// id や id プレフィックスが困ることは一切ない。複数行がマッチした場合は
    /// あいまいとして扱わず id 順で最初の1件を返す。これは置き換え元の Node
    /// 製 MCP サーバの挙動を踏襲したもので、明示的な ORDER BY によって
    /// 決定的にしている点だけが異なる。
    pub fn resolve_id_prefix(&self, prefix: &str) -> Result<Option<String>> {
        // ツール側は「ID または一意なプレフィックス（最短8文字）」と公表している。
        // それをドキュメントに書くだけでなく実際に強制する。1〜2文字の
        // プレフィックスだと、たまたま id順で最初に来たものにマッチしてしまい、
        // id を打ち間違えたモデルが他人のコメントを解決・返信してしまい、
        // しかも成功したと報告されることになる。
        if prefix.len() < MIN_ID_PREFIX_LEN {
            return Ok(None);
        }
        // id は UUID なので、16進数とハイフン以外の文字を含むものは絶対に
        // マッチしない。ここで拒否しておけば、LIKE のワイルドカードである
        // % と _ が下のパターンに紛れ込んで無関係な行にマッチすることも防げる。
        if !prefix.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
            return Ok(None);
        }
        let pattern = format!("{prefix}%");
        let result = self.conn.query_row(
            "SELECT id FROM reviews WHERE id LIKE ?1 ORDER BY id LIMIT 1",
            params![pattern],
            |row| row.get(0),
        );
        match result {
            Ok(id) => Ok(Some(id)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// 未解決のレビューコメントを返す。branch、worktree、file_path で
    /// 任意に絞り込める。
    ///
    /// branch は branch カラムまたは worktree カラムのどちらかにマッチする
    /// （OR で、両方とも同じ値にバインドする）。v4 スキーマの CHECK
    /// （branch IS NULL OR worktree = branch）の下では、非 null な branch は
    /// 常に既に worktree と一致しているため、branch = ? 側だけがマッチの
    /// 決め手になることは起こり得ず、この OR は現行スキーマ上は冗長になる。
    /// それでも残しているのは、置き換え元である Node 製 MCP サーバとの
    /// 互換性のためで、あちらはこの CHECK が入る前から存在し、両者が
    /// 食い違う行が見えることもあった。
    pub fn pending_reviews(
        &self,
        branch: Option<&str>,
        worktree: Option<&str>,
        file_path: Option<&str>,
    ) -> Result<Vec<ReviewComment>> {
        let mut sql = String::from(
            "SELECT id, worktree, file_path, line_start, line_end, kind, body, status,
                    author, branch, created_at
             FROM reviews WHERE status = 'pending'",
        );
        let mut bind: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(w) = worktree {
            sql.push_str(" AND worktree = ?");
            bind.push(Box::new(w.to_string()));
        }
        if let Some(b) = branch {
            sql.push_str(" AND (branch = ? OR worktree = ?)");
            bind.push(Box::new(b.to_string()));
            bind.push(Box::new(b.to_string()));
        }
        if let Some(f) = file_path {
            sql.push_str(" AND file_path = ?");
            bind.push(Box::new(f.to_string()));
        }
        sql.push_str(" ORDER BY file_path, line_start");

        let mut stmt = self.conn.prepare(&sql)?;
        collect_reviews(&mut stmt, rusqlite::params_from_iter(bind.iter()))
    }
}

/// rusqlite::Row を ReviewComment に変換する。
///
/// 想定しているカラム順（11カラム）:
///   0:id, 1:worktree, 2:file_path, 3:line_start, 4:line_end,
///   5:kind, 6:body, 7:status, 8:author, 9:branch, 10:created_at
fn row_to_review(row: &rusqlite::Row<'_>) -> rusqlite::Result<ReviewComment> {
    let kind_str: String = row.get(5)?;
    let status_str: String = row.get(7)?;
    let author_str: String = row.get(8)?;

    let kind = match kind_str.as_str() {
        "suggest" => CommentKind::Suggest,
        "question" => CommentKind::Question,
        other => {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                5,
                rusqlite::types::Type::Text,
                format!("unknown CommentKind: {other}").into(),
            ));
        }
    };

    let status = match status_str.as_str() {
        "pending" => CommentStatus::Pending,
        "resolved" => CommentStatus::Resolved,
        other => {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                7,
                rusqlite::types::Type::Text,
                format!("unknown CommentStatus: {other}").into(),
            ));
        }
    };

    let author = match author_str.as_str() {
        "user" => Author::User,
        "claude" => Author::Claude,
        other => {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                8,
                rusqlite::types::Type::Text,
                format!("unknown Author: {other}").into(),
            ));
        }
    };

    Ok(ReviewComment {
        id: row.get(0)?,
        worktree: row.get(1)?,
        file_path: row.get(2)?,
        line_start: row.get::<_, i64>(3)? as u32,
        line_end: row.get::<_, Option<i64>>(4)?.map(|n| n as u32),
        kind,
        body: row.get(6)?,
        status,
        author,
        branch: row.get(9)?,
        created_at: row.get(10)?,
    })
}

/// 準備済みステートメントを実行し、マッチした全行を Vec<ReviewComment> に集める。
pub(super) fn collect_reviews(
    stmt: &mut rusqlite::Statement<'_>,
    params: impl rusqlite::Params,
) -> Result<Vec<ReviewComment>> {
    let rows = stmt.query_map(params, row_to_review)?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::super::test_support::test_store;
    use super::*;

    #[test]
    fn add_and_retrieve_review() {
        let store = test_store();

        let review = store
            .add_review(
                "wt1",
                "src/main.rs",
                42,
                None,
                CommentKind::Suggest,
                "use guard clause",
                "abc123",
                Author::User,
                None,
            )
            .unwrap();

        assert_eq!(review.worktree, "wt1");
        assert_eq!(review.file_path, "src/main.rs");
        assert_eq!(review.line_start, 42);
        assert_eq!(review.line_end, None);
        assert_eq!(review.kind, CommentKind::Suggest);
        assert_eq!(review.body, "use guard clause");
        assert_eq!(review.status, CommentStatus::Pending);
        assert_eq!(review.author, Author::User);
        assert_eq!(review.branch, None);

        // worktree で取得する
        let reviews = store.reviews_for_worktree("wt1").unwrap();
        assert_eq!(reviews.len(), 1);
        assert_eq!(reviews[0].id, review.id);
    }

    #[test]
    fn update_body() {
        let store = test_store();

        let review = store
            .add_review(
                "wt1",
                "src/app.rs",
                5,
                None,
                CommentKind::Suggest,
                "original",
                "abc",
                Author::User,
                None,
            )
            .unwrap();

        store.update_review_body(&review.id, "edited").unwrap();
        let reviews = store.reviews_for_worktree("wt1").unwrap();
        assert_eq!(reviews[0].body, "edited");
    }

    #[test]
    fn line_range_and_author() {
        let store = test_store();

        // worktree と branch には同じブランチ名を持たせる（v4 の CHECK が強制
        // している）。コメントのカラムにも両方に格納される。
        let review = store
            .add_review(
                "feature/x",
                "src/main.rs",
                10,
                Some(20),
                CommentKind::Suggest,
                "refactor this block",
                "abc",
                Author::Claude,
                Some("feature/x"),
            )
            .unwrap();

        assert_eq!(review.line_start, 10);
        assert_eq!(review.line_end, Some(20));
        assert_eq!(review.author, Author::Claude);
        assert_eq!(review.branch.as_deref(), Some("feature/x"));

        // 単一行（line_end = None）
        let r2 = store
            .add_review(
                "wt1",
                "src/main.rs",
                5,
                None,
                CommentKind::Question,
                "why?",
                "abc",
                Author::User,
                None,
            )
            .unwrap();
        assert_eq!(r2.line_start, 5);
        assert_eq!(r2.line_end, None);
        assert_eq!(r2.author, Author::User);
        assert_eq!(r2.branch, None);
    }

    #[test]
    fn mark_published_hides_reviews_from_unpublished_query() {
        let store = test_store();

        let r1 = store
            .add_review(
                "feat/x",
                "src/main.rs",
                1,
                None,
                CommentKind::Suggest,
                "first",
                "abc123",
                Author::User,
                Some("feat/x"),
            )
            .unwrap();
        let r2 = store
            .add_review(
                "feat/x",
                "src/lib.rs",
                2,
                None,
                CommentKind::Question,
                "second",
                "abc123",
                Author::User,
                Some("feat/x"),
            )
            .unwrap();

        let unpublished = store.unpublished_reviews("feat/x").unwrap();
        assert_eq!(unpublished.len(), 2);

        store
            .mark_published(std::slice::from_ref(&r1.id), "2026-07-05T00:00:00Z")
            .unwrap();

        let unpublished = store.unpublished_reviews("feat/x").unwrap();
        assert_eq!(unpublished.len(), 1);
        assert_eq!(unpublished[0].id, r2.id);
    }

    #[test]
    fn pending_reviews_filters_by_status() {
        let store = test_store();

        let pending = store
            .add_review(
                "wt1",
                "src/a.rs",
                1,
                None,
                CommentKind::Suggest,
                "still open",
                "abc",
                Author::User,
                None,
            )
            .unwrap();
        let resolved = store
            .add_review(
                "wt1",
                "src/b.rs",
                2,
                None,
                CommentKind::Suggest,
                "done",
                "abc",
                Author::User,
                None,
            )
            .unwrap();
        store
            .update_review_status(&resolved.id, CommentStatus::Resolved)
            .unwrap();

        let rows = store.pending_reviews(None, None, None).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, pending.id);
    }

    /// (branch = ? OR worktree = ?) 句を検証する。v4 の CHECK
    /// （branch IS NULL OR worktree = branch）の下では、branch = ? 単独が
    /// マッチの決め手になることはない（非 null な branch は常に既に
    /// worktree と一致している）。そのためこのテストは worktree = ? 側
    /// （via_worktree、branch は NULL）だけを切り分けている。via_branch の
    /// 行はたまたま worktree = ? も満たしてしまうため、branch = ? 側が
    /// このスキーマ上で何か効いていることの証明にはならない。それでも
    /// branch フィルタ "feat/x" に対しては両方の行が返ってこなければ
    /// ならない。
    #[test]
    fn pending_reviews_matches_branch_or_worktree_column() {
        let store = test_store();

        // v4 の CHECK（branch IS NULL OR worktree = branch）は worktree を
        // 非 null な branch と一致させることを強制するため、この行は両方の
        // カラムにマッチする。worktree のみの経路を切り分けているのは
        // 下の行の方。
        let via_branch = store
            .add_review(
                "feat/x",
                "src/a.rs",
                1,
                None,
                CommentKind::Suggest,
                "matches via branch",
                "abc",
                Author::User,
                Some("feat/x"),
            )
            .unwrap();
        let via_worktree = store
            .add_review(
                "feat/x",
                "src/b.rs",
                2,
                None,
                CommentKind::Suggest,
                "matches via worktree",
                "abc",
                Author::User,
                None,
            )
            .unwrap();

        let rows = store.pending_reviews(Some("feat/x"), None, None).unwrap();
        let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(rows.len(), 2);
        assert!(ids.contains(&via_branch.id.as_str()));
        assert!(ids.contains(&via_worktree.id.as_str()));
    }

    #[test]
    fn pending_reviews_filters_by_file_path() {
        let store = test_store();

        let a = store
            .add_review(
                "wt1",
                "src/a.rs",
                1,
                None,
                CommentKind::Suggest,
                "on a.rs",
                "abc",
                Author::User,
                None,
            )
            .unwrap();
        store
            .add_review(
                "wt1",
                "src/b.rs",
                2,
                None,
                CommentKind::Suggest,
                "on b.rs",
                "abc",
                Author::User,
                None,
            )
            .unwrap();

        let rows = store.pending_reviews(None, None, Some("src/a.rs")).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, a.id);
    }

    #[test]
    fn resolve_id_prefix_finds_by_8char_prefix() {
        let store = test_store();

        let review = store
            .add_review(
                "wt1",
                "src/a.rs",
                1,
                None,
                CommentKind::Suggest,
                "note",
                "abc",
                Author::User,
                None,
            )
            .unwrap();

        let resolved = store
            .resolve_id_prefix(&review.id[..8])
            .unwrap()
            .expect("prefix should resolve");
        assert_eq!(resolved, review.id);
    }

    #[test]
    fn resolve_id_prefix_returns_none_when_no_match() {
        let store = test_store();
        store
            .add_review(
                "wt1",
                "src/a.rs",
                1,
                None,
                CommentKind::Suggest,
                "note",
                "abc",
                Author::User,
                None,
            )
            .unwrap();

        assert_eq!(store.resolve_id_prefix("deadbeef").unwrap(), None);
    }

    /// プレフィックスは16進数と - だけを通す。検証が緩むと、id 順で最初に
    /// 来たコメントに解決されてしまう。セキュリティ上重要な抜け穴。
    #[test]
    fn resolve_id_prefix_rejects_invalid_prefixes() {
        let store = test_store();
        store
            .add_review(
                "wt1",
                "src/a.rs",
                1,
                None,
                CommentKind::Suggest,
                "note",
                "abc",
                Author::User,
                None,
            )
            .unwrap();

        for (prefix, why) in [
            ("%", "LIKE の任意長ワイルドカード"),
            (
                "_",
                "LIKE の1文字ワイルドカード。% を潰しただけでは塞げない",
            ),
            ("", "空文字"),
            (
                "xyz",
                "%/_ を含まないので、ワイルドカードの除去だけを実装したバリデータは素通りさせてしまう",
            ),
        ] {
            assert_eq!(store.resolve_id_prefix(prefix).unwrap(), None, "{why}");
        }
    }

    #[test]
    fn resolve_id_prefix_is_deterministic_with_multiple_matches() {
        let store = test_store();

        // プレフィックスを共有するように手作りした id。実際の UUID は
        // ランダムなので、あいまいなプレフィックスを確実に起こす唯一の方法が
        // これになる。意図的に id の降順で挿入している。昇順で挿入すると、
        // rowid の順序（ORDER BY がない場合の SQLite のデフォルト）が id の
        // 順序と一致してしまい、クエリから ORDER BY id を外しても
        // テストが通ってしまう。
        for id in [
            "aaaaaaaa-2222-0000-0000-000000000000",
            "aaaaaaaa-1111-0000-0000-000000000000",
        ] {
            store
                .conn
                .execute(
                    "INSERT INTO reviews (id, worktree, file_path, line_start, kind, body, commit_ref)
                     VALUES (?1, 'wt1', 'src/a.rs', 1, 'suggest', 'note', 'abc')",
                    params![id],
                )
                .unwrap();
        }

        let resolved = store.resolve_id_prefix("aaaaaaaa").unwrap().unwrap();
        assert_eq!(resolved, "aaaaaaaa-1111-0000-0000-000000000000");
    }

    /// 公表している8文字より短いプレフィックスは何にも解決されない。この
    /// チェックがなければ、"a" のような打ち間違いの id が、id順で最初に
    /// 来たコメントにサイレントにマッチしてしまい、他人のコメントを
    /// 解決・返信した上で成功したと報告することになる。
    #[test]
    fn resolve_id_prefix_rejects_prefixes_shorter_than_advertised() {
        let store = test_store();
        let review = store
            .add_review(
                "wt1",
                "src/a.rs",
                1,
                None,
                CommentKind::Suggest,
                "note",
                "abc",
                Author::User,
                None,
            )
            .unwrap();

        // 実在する id の先頭文字そのものであっても、それでも拒否される。
        for len in 1..MIN_ID_PREFIX_LEN {
            let short_prefix = &review.id[..len];
            assert_eq!(
                store.resolve_id_prefix(short_prefix).unwrap(),
                None,
                "{len}-char prefix must not resolve"
            );
        }
        // 公表している長さちょうどなら解決される。ここでテストしているのは
        // その境界だけである。
        assert_eq!(
            store
                .resolve_id_prefix(&review.id[..MIN_ID_PREFIX_LEN])
                .unwrap(),
            Some(review.id.clone())
        );
    }
}
