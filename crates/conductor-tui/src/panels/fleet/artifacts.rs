//! 一覧の下区画: 選んだセッションが publish した Artifact。

use std::path::PathBuf;
use std::time::{Duration, Instant};

use conductor_core::claude_log::artifacts::{Artifact, Scan};

use crate::click::ClickTracker;

/// 同じセッションを読み直すまでの間。読むのは伸びた分だけなので短くてよい。
pub const REFRESH: Duration = Duration::from_secs(2);

/// ログを引く会話の id と、Claude がそのとき居る dir。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conversation {
    pub id: String,
    pub cwd: PathBuf,
}

#[derive(Debug, Default)]
pub struct SessionArtifacts {
    shown: Option<(String, Scan)>,
    fetching: bool,
    landed: Option<Instant>,
    /// 連打でタブが何枚も開かないように、開き終わるまで次を捨てる。
    opening: bool,
    clicks: ClickTracker<(u16, String)>,
}

impl SessionArtifacts {
    /// 投げ中は何も返さない。
    pub fn wanted(
        &self,
        target: Option<Conversation>,
        now: Instant,
    ) -> Option<(Conversation, Scan)> {
        let target = target?;
        if self.fetching {
            return None;
        }
        let from = match &self.shown {
            Some((id, scan)) if *id == target.id => {
                if self.landed.is_some_and(|at| now < at + REFRESH) {
                    return None;
                }
                scan.clone()
            }
            _ => Scan::default(),
        };
        Some((target, from))
    }

    pub fn note_fetching(&mut self) {
        self.fetching = true;
    }

    pub fn forget_in_flight(&mut self) {
        self.fetching = false;
        self.opening = false;
    }

    pub fn install(&mut self, id: String, scan: Option<Scan>, target: Option<&str>, now: Instant) {
        self.fetching = false;
        if target != Some(id.as_str()) {
            return;
        }
        let scan = match (scan, self.shown.take()) {
            (Some(scan), _) => scan,
            (None, Some((shown, kept))) if shown == id => kept,
            (None, _) => Scan::default(),
        };
        self.shown = Some((id, scan));
        self.landed = Some(now);
    }

    pub fn for_target(&self, target: Option<&str>) -> Option<&[Artifact]> {
        let (id, scan) = self.shown.as_ref()?;
        (target == Some(id.as_str())).then_some(scan.list.as_slice())
    }

    pub fn click(&mut self, line: u16, target: Option<&str>) -> Option<String> {
        let url = self.for_target(target)?.get(usize::from(line))?.url.clone();
        if !self.clicks.is_double((line, url.clone())) || self.opening {
            return None;
        }
        // 三度目の押下が、開き終わった直後の二度目として数えられないように。
        self.clicks = ClickTracker::default();
        self.opening = true;
        Some(url)
    }

    /// 開き終わる (note_opened) までは false を返す。
    pub fn start_opening(&mut self) -> bool {
        !std::mem::replace(&mut self.opening, true)
    }

    pub fn note_opened(&mut self) {
        self.opening = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn talk(id: &str) -> Conversation {
        Conversation {
            id: id.into(),
            cwd: PathBuf::from("/r"),
        }
    }

    fn scan_of(urls: &[&str]) -> Scan {
        Scan {
            offset: 10,
            list: urls
                .iter()
                .map(|url| Artifact {
                    url: url.to_string(),
                    title: url.to_string(),
                })
                .collect(),
        }
    }

    fn landed_on(id: &str, scan: Scan, now: Instant) -> SessionArtifacts {
        let mut artifacts = SessionArtifacts::default();
        artifacts.note_fetching();
        artifacts.install(id.into(), Some(scan), Some(id), now);
        artifacts
    }

    #[test]
    fn 投げ中は別のセッションを選んでも次を投げない() {
        let mut artifacts = SessionArtifacts::default();
        artifacts.note_fetching();
        assert_eq!(artifacts.wanted(Some(talk("b")), Instant::now()), None);
    }

    #[test]
    fn 同じセッションは届いてから間をあけ_続きから読む() {
        let now = Instant::now();
        let artifacts = landed_on("a", scan_of(&["u"]), now);
        assert_eq!(artifacts.wanted(Some(talk("a")), now), None);
        assert_eq!(
            artifacts.wanted(Some(talk("a")), now + REFRESH),
            Some((talk("a"), scan_of(&["u"])))
        );
    }

    #[test]
    fn 別のセッションは待たずに頭から読む() {
        let now = Instant::now();
        let artifacts = landed_on("a", scan_of(&["u"]), now);
        assert_eq!(
            artifacts.wanted(Some(talk("b")), now),
            Some((talk("b"), Scan::default()))
        );
    }

    #[test]
    fn 選択が移った後に届いた結果は据えず_投げ中だけ下ろす() {
        let now = Instant::now();
        let mut artifacts = landed_on("a", scan_of(&["u"]), now);
        artifacts.note_fetching();
        artifacts.install("a".into(), Some(scan_of(&["v"])), Some("b"), now);
        assert!(artifacts.for_target(Some("b")).is_none());
        assert!(artifacts.wanted(Some(talk("b")), now).is_some());
    }

    #[test]
    fn 読めなかったときは前の結果を残し_時刻は進める() {
        let now = Instant::now();
        let mut artifacts = landed_on("a", scan_of(&["u"]), now);
        artifacts.note_fetching();
        let later = now + REFRESH;
        artifacts.install("a".into(), None, Some("a"), later);
        assert_eq!(artifacts.for_target(Some("a")).unwrap()[0].url, "u");
        assert_eq!(artifacts.wanted(Some(talk("a")), later), None);
    }

    #[test]
    fn repo_を切り替えて結果が捨てられても次を投げられる() {
        let mut artifacts = SessionArtifacts::default();
        artifacts.note_fetching();
        artifacts.forget_in_flight();
        assert!(artifacts.wanted(Some(talk("a")), Instant::now()).is_some());
    }

    #[test]
    fn 同じ行の二度目で開き_開き終わるまで次は開かない() {
        let mut artifacts = landed_on("a", scan_of(&["u", "v"]), Instant::now());
        assert_eq!(artifacts.click(1, Some("a")), None);
        assert_eq!(artifacts.click(1, Some("a")), Some("v".into()));
        assert_eq!(artifacts.click(1, Some("a")), None);
        artifacts.note_opened();
        artifacts.click(0, Some("a"));
        assert_eq!(artifacts.click(0, Some("a")), Some("u".into()));
    }

    #[test]
    fn 開き終わった直後の三度目の押下では開かない() {
        let mut artifacts = landed_on("a", scan_of(&["u"]), Instant::now());
        artifacts.click(0, Some("a"));
        assert_eq!(artifacts.click(0, Some("a")), Some("u".into()));
        artifacts.note_opened();
        assert_eq!(artifacts.click(0, Some("a")), None);
    }

    #[test]
    fn 別のセッションの結果しか無ければ押しても開かない() {
        let mut artifacts = landed_on("a", scan_of(&["u"]), Instant::now());
        artifacts.click(0, Some("b"));
        assert_eq!(artifacts.click(0, Some("b")), None);
    }

    #[test]
    fn 行の無い所を押しても開かない() {
        let mut artifacts = landed_on("a", scan_of(&["u"]), Instant::now());
        artifacts.click(5, Some("a"));
        assert_eq!(artifacts.click(5, Some("a")), None);
    }
}
