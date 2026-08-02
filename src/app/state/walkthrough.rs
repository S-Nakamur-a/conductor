//! AI ウォークスルーの状態: 生成中のものと、いま読み込まれているもの。

use crate::app::WalkthroughGenerations;
use crate::walkthrough::{Walkthrough, WalkthroughStep};

/// 読み込み済みのウォークスルー 1 本 (ヘッダ + ステップ列)。
///
/// ストアからはタプルで返ってくるが、利用側はほぼ必ず片方だけを使うので
/// ((_, steps) / (w, _) の分解が繰り返されていた) 名前を付けている。
pub struct LoadedWalkthrough {
    /// タイトルや生成状態を持つヘッダ。
    pub header: Walkthrough,
    /// intent -> core -> ripple -> test の順に並んだステップ。
    pub steps: Vec<WalkthroughStep>,
}

impl From<(Walkthrough, Vec<WalkthroughStep>)> for LoadedWalkthrough {
    fn from((header, steps): (Walkthrough, Vec<WalkthroughStep>)) -> Self {
        Self { header, steps }
    }
}

/// ウォークスルーの生成と表示の状態。
#[derive(Default)]
pub struct WalkthroughState {
    /// 生成中のウォークスルー。ブランチごとに高々 1 本なので worktree 同士が
    /// 待ち合わせにならない。各要素はバックグラウンドスレッドの結果チャネルと、
    /// 正しいブランチに結果を返すための最低限の文脈。
    /// [crate::app::App::poll_walkthrough_generation] が回収する。
    pub generations: WalkthroughGenerations,
    /// 選択中の worktree のウォークスルー。
    /// [crate::app::App::refresh_reviews] がコメント一覧と一緒に読み直す。
    pub current: Option<LoadedWalkthrough>,
}
