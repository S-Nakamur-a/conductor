//! アニメーションの時間勘定。

/// フレームごとに進む 2 つのカウンタ。
///
/// どちらも折り返す。飽和させると、長く開けたままのウィンドウでスピナーや
/// 点滅がある日いきなり止まる。
#[derive(Default)]
pub struct Ticks {
    ui: u64,
    decoration: u64,
}

impl Ticks {
    pub fn ui(&self) -> u64 {
        self.ui
    }

    /// 装飾は別勘定。UI のフレームより粗い間隔で進む。
    pub fn decoration(&self) -> u64 {
        self.decoration
    }

    pub fn advance_ui(&mut self) {
        self.ui = self.ui.wrapping_add(1);
    }

    pub fn advance_decoration(&mut self) {
        self.decoration = self.decoration.wrapping_add(1);
    }

    /// tick は折り返すので、引き算は必ずこちらを通す。
    pub fn ui_since(&self, then: u64) -> u64 {
        self.ui.wrapping_sub(then)
    }
}
