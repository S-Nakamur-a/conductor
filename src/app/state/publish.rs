//! レビューコメントの GitHub 公開フローの状態。

use crate::background::BackgroundOp;
use crate::review_publish::{PublishConfirm, PublishOutcome};

/// 公開の確認待ちと、実行中の公開処理。
#[derive(Default)]
pub struct PublishState {
    /// `Action::PublishReview` の y/n 確認待ち。
    ///
    /// 確認オーバーレイが出ているあいだだけ `Some` で、表示に必要な
    /// フィルタ済みコメントとスキップ件数を抱えている。どちらの答えでも消える。
    pub confirm: Option<PublishConfirm>,
    /// 実行中の GitHub 公開処理。
    pub op: BackgroundOp<PublishOutcome>,
}
