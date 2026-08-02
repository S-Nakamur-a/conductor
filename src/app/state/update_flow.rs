//! 自己更新フロー (新バージョンの検出 → 確認 → インストール → 再起動) の状態。

use std::path::PathBuf;

use crate::app::update::{UpdateProgress, UpdateState};
use crate::background::BackgroundOp;
use crate::update_checker::UpdateInfo;

/// 更新チェックから再起動までを 1 本のフローとして持つ状態。
///
/// startup_exe / startup_args / should_restart もここに含めているのは、
/// 再起動がこのフローの最後の一歩だから。再起動は exec でプロセスイメージを
/// 置き換えるので、main はループを抜けたあとにこの 3 つを読む。
#[derive(Default)]
pub struct UpdateFlow {
    /// より新しいバージョンが見つかったときのリリース情報。
    pub info: Option<UpdateInfo>,
    /// コマンドパレットから手動でチェックを起動したか。
    ///
    /// 起動時のサイレントチェックが握り潰す「最新版です」「チェック失敗」も
    /// 明示的にフィードバックするための印。
    pub check_requested: bool,
    /// 更新フローの現在位置。
    pub state: UpdateState,
    /// バックグラウンドで走っているダウンロード / ビルド。
    pub op: BackgroundOp<UpdateProgress>,
    /// オーバーレイに表示する最新の進捗メッセージ。
    pub progress_message: String,
    /// タイトルバー上の更新バッジの桁範囲 (クリック判定用)。
    pub badge_cols: Option<(u16, u16)>,
    /// 起動時の実行ファイルパス (exec による再起動に使う)。
    pub startup_exe: PathBuf,
    /// 起動時のコマンドライン引数 (exec による再起動に使う)。
    pub startup_args: Vec<String>,
    /// 更新が完了し、再起動すべき状態になったら true。
    pub should_restart: bool,
}

impl UpdateFlow {
    /// 起動時の実行ファイルと引数を捕まえた初期状態を作る。
    pub fn from_current_process() -> Self {
        Self {
            startup_exe: std::env::current_exe().unwrap_or_default(),
            startup_args: std::env::args().skip(1).collect(),
            ..Self::default()
        }
    }

    /// 更新オーバーレイが画面に出ているか (= 通常操作を奪っているか)。
    pub fn is_active(&self) -> bool {
        self.state != UpdateState::Idle
    }
}
