//! リッチモード (端末グラフィックス) の状態。

use crate::term_caps::RichTier;

/// 起動時の端末ケイパビリティ検出で決まる描画ティアと、それに紐づく資源。
///
/// `tier` は実行時トグルで `Off` に落とせるので、検出結果そのものは
/// `available` に別で保持する — トグルを戻したときの復帰先になる。
pub struct RichState {
    /// 現在有効なティア。config と端末検出から起動時に決まり、
    /// 実行時トグルで `Off` との間を行き来する。
    pub tier: RichTier,
    /// Tier B の画像描画に使うグラフィックスプロトコル picker。
    /// `tier` が Tier B のときだけ `Some`。
    pub picker: Option<ratatui_image::picker::Picker>,
    /// 起動時に検出できたティア。実行時トグルで `tier` を落としても
    /// ここは変わらないので、トグルを戻すときの復帰先として使う。
    pub available: RichTier,
    /// リッチモードのアニメーションの基準時刻。
    ///
    /// 位相を `ui_tick` ではなく経過時間から導くための起点。再描画レートは
    /// アイドル時の約 2fps から入力中の約 60fps まで変動するので、tick 基準
    /// だとアニメーション速度が描画レートに引きずられてしまう。
    pub epoch: std::time::Instant,
}

impl Default for RichState {
    fn default() -> Self {
        Self {
            tier: RichTier::Off,
            picker: None,
            available: RichTier::Off,
            epoch: std::time::Instant::now(),
        }
    }
}

impl RichState {
    /// なんらかのリッチ効果 (Tier A 以上) が有効か。
    pub fn is_rich(&self) -> bool {
        self.tier.is_rich()
    }

    /// グラフィックスプロトコルによる画像描画 (Tier B) が有効か。
    pub fn has_graphics(&self) -> bool {
        self.tier.has_graphics()
    }
}
