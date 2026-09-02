//! 再起動なしで反映できる外観フィールドと、再起動が要るフィールドの切り分け。
//!
//! どちらに属するかは [Config::adopt_appearance] だけが決める。写すフィールドが live、
//! 写さないフィールドが restart。スナップショットも再起動判定もそこから導くので、
//! フィールドを足したときに片方のリストだけ更新し忘れることがない。

use super::Config;

/// live フィールドだけを持つ Config。restart フィールドは既定値に潰してあるので、
/// 等価比較が外観の差だけを見る。
#[derive(Debug, Clone, PartialEq)]
pub struct AppearanceSnapshot(Config);

impl Config {
    pub fn appearance_snapshot(&self) -> AppearanceSnapshot {
        let mut live = Config::default();
        live.adopt_appearance(self);
        AppearanceSnapshot(live)
    }

    /// 外観フィールドを new から写す。restart フィールドは触らない。
    pub fn adopt_appearance(&mut self, new: &Config) {
        self.ui = new.ui.clone();
        self.viewer = new.viewer.clone();
        self.diff = new.diff.clone();
        self.layout = new.layout.clone();
    }
}

/// old と new が restart フィールドのどれかで異なれば true。
pub fn has_restart_changes(old: &Config, new: &Config) -> bool {
    let mut merged = old.clone();
    merged.adopt_appearance(new);
    merged != *new
}
