//! Alt+/ で出すパネル番号バッジのオーバーレイ。

use std::time::{Duration, Instant};

/// 表示してから自動的に消えるまでの時間。
const AUTO_DISMISS: Duration = Duration::from_secs(2);

/// 各パネルの上に番号バッジを重ねる一時オーバーレイ。
///
/// 「立てたフラグ」と「立てた時刻」は必ず一緒に読む必要がある — フラグだけ見て
/// 表示すると、自動消灯の時刻を過ぎても出しっぱなしになる。両方を包んで
/// [`Self::is_visible`] からしか見えないようにしてある。
#[derive(Default)]
pub struct PanelNumberOverlay {
    /// トグルで立てられたか。時間切れの判定は含まないので、表示可否は
    /// このフィールドではなく [`Self::is_visible`] で判断する。
    requested: bool,
    /// 立てた時刻 (自動消灯のタイマー)。
    since: Option<Instant>,
}

impl PanelNumberOverlay {
    /// いま描画すべきか。立ててから [`AUTO_DISMISS`] 経過すると `false` に戻る。
    pub fn is_visible(&self) -> bool {
        self.requested && self.since.is_some_and(|t| t.elapsed() < AUTO_DISMISS)
    }

    /// トグルする。立てるときは自動消灯のタイマーも開始する。
    pub fn toggle(&mut self) {
        if self.is_visible() {
            self.hide();
        } else {
            self.requested = true;
            self.since = Some(Instant::now());
        }
    }

    /// 消す。
    pub fn hide(&mut self) {
        self.requested = false;
        self.since = None;
    }

    /// 時間切れになった表示済みオーバーレイを片付ける。
    ///
    /// 片付けた (= 状態が変わった) なら `true`。イベントループが再描画の
    /// 要否を決めるのに使う。
    pub fn expire_if_due(&mut self) -> bool {
        if self.requested && !self.is_visible() {
            self.hide();
            return true;
        }
        false
    }
}
