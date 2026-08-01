//! セッション統計 (ゲーミフィケーション) と ccusage のキャッシュ。

use crate::app::types::CcusageInfo;
use crate::review_store::DailyStats;

/// 稼働状況の集計値。表示専用で、どれも失われても機能は壊れない
/// (統計 DB が開けなければ全部 `None` のまま動く)。
#[derive(Default)]
pub struct SessionStats {
    /// 現在の統計セッション ID。
    pub session_id: Option<String>,
    /// 今日の活動統計のキャッシュ (定期的に更新)。
    pub today: Option<DailyStats>,
    /// ccusage (トークン / コスト) のキャッシュ。
    /// バックグラウンドスレッドで定期的に更新される。
    pub ccusage: Option<CcusageInfo>,
}
