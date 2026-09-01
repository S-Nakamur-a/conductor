//! アニメーションの時間勘定。

/// フレームごとに進むカウンタ。
///
/// 折り返す。飽和させると、長く開けたままのウィンドウでスピナーや点滅が
/// ある日いきなり止まる。
#[derive(Default)]
pub struct Ticks {
    ui: u64,
}

impl Ticks {
    pub fn ui(&self) -> u64 {
        self.ui
    }

    pub fn advance_ui(&mut self) {
        self.ui = self.ui.wrapping_add(1);
    }

    /// tick は折り返すので、引き算は必ずこちらを通す。
    pub fn ui_since(&self, then: u64) -> u64 {
        self.ui.wrapping_sub(then)
    }
}
