use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use conductor_core::claude_log::summary::{Summary, Title};
use conductor_core::claude_sessions::registry::{LiveSession, Owner, Status, Waiting};
use conductor_core::keymap::Action;

use super::*;
use crate::workspace::Workspace;

fn at(secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_792_000_000 + secs)
}

fn live(pid: u32, cwd: &str, session: &str, status: Status, owner: Owner) -> LiveSession {
    LiveSession {
        pid,
        cwd: PathBuf::from(cwd),
        session_id: session.to_string(),
        status,
        status_since: Some(at(100)),
        owner,
        started: at(u64::from(pid)),
    }
}

fn own(panel: &str, cwd: &str, session: Option<&str>) -> OwnSession {
    OwnSession {
        panel_id: panel.to_string(),
        working_dir: PathBuf::from(cwd),
        claude_session_id: session.map(str::to_string),
    }
}

fn status(live: Vec<LiveSession>) -> FleetStatus {
    FleetStatus {
        live,
        summaries: Vec::new(),
    }
}

fn keys(fleet: &FleetPanel) -> Vec<(RowKey, bool)> {
    fleet
        .visual_rows()
        .into_iter()
        .map(|r| (r.key, r.pinned))
        .collect()
}

#[test]
fn この窓のセッションは状態ファイルと会話の_id_で突き合わせる() {
    let mut fleet = FleetPanel::default();
    fleet.apply(
        status(vec![live(
            7,
            "/r/a",
            "s1",
            Status::Working,
            Owner::Conductor,
        )]),
        vec![own("CC:1", "/r/a", Some("s1"))],
        None,
        false,
        at(200),
    );
    let rows = fleet.visual_rows();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].key, RowKey::Panel("CC:1".into()));
    assert_eq!(rows[0].place, Place::Mine);
    assert_eq!(rows[0].state, State::Working);
}

#[test]
fn 状態ファイルの無いこの窓のセッションも状態不明で出る() {
    let mut fleet = FleetPanel::default();
    fleet.apply(
        status(Vec::new()),
        vec![own("CC:1", "/r/a", None)],
        None,
        false,
        at(200),
    );
    let rows = fleet.visual_rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].state, State::Unknown);
}

#[test]
fn この窓の外のセッションは_pid_で持ち_外の端末は最後の欄() {
    let mut fleet = FleetPanel::default();
    fleet.apply(
        status(vec![
            live(9, "/r/zzz", "s9", Status::Idle, Owner::Elsewhere),
            live(8, "/r/b", "s8", Status::Working, Owner::Conductor),
        ]),
        Vec::new(),
        None,
        false,
        at(200),
    );
    let (_, groups) = fleet.groups();
    let names: Vec<&str> = groups.iter().map(|g| g.name.as_str()).collect();
    assert_eq!(names, ["b", "外の端末"]);
    assert_eq!(groups[0].rows[0].key, RowKey::Pid(8));
    assert_eq!(groups[0].rows[0].place, Place::OtherConductor);
    assert_eq!(groups[1].rows[0].place, Place::Outside);
}

#[test]
fn 入力待ちは一番上にも_repo_の欄にもいる() {
    let mut fleet = FleetPanel::default();
    fleet.apply(
        status(vec![
            live(
                7,
                "/r/a",
                "s1",
                Status::Waiting(Waiting::Question),
                Owner::Conductor,
            ),
            live(8, "/r/a", "s2", Status::Working, Owner::Conductor),
        ]),
        Vec::new(),
        None,
        false,
        at(200),
    );
    assert_eq!(
        keys(&fleet),
        [
            (RowKey::Pid(7), true),
            (RowKey::Pid(7), false),
            (RowKey::Pid(8), false),
        ]
    );
}

#[test]
fn 止まったものは選ぶか映すまで完了_未確認() {
    let mut fleet = FleetPanel::default();
    let idle = || {
        status(vec![
            live(7, "/r/a", "s1", Status::Working, Owner::Conductor),
            live(8, "/r/a", "s2", Status::Idle, Owner::Conductor),
        ])
    };
    fleet.apply(idle(), Vec::new(), None, false, at(50));
    let state_of = |fleet: &FleetPanel, pid| {
        fleet
            .visual_rows()
            .into_iter()
            .find(|r| r.key == RowKey::Pid(pid))
            .unwrap()
            .state
    };
    // 先頭 (pid 7) が選ばれ、pid 8 はまだ誰も見ていない。
    assert_eq!(state_of(&fleet, 8), State::Finished);
    let ws = Workspace::for_test();
    fleet.update(Action::NavigateDown, &ws.ctx());
    assert_eq!(state_of(&fleet, 8), State::Idle, "選んだら待機");
}

#[test]
fn 今の画面に映っていたこの窓のセッションは待機() {
    let mut fleet = FleetPanel::default();
    fleet.apply(
        status(vec![
            live(6, "/r/a", "s0", Status::Working, Owner::Conductor),
            live(7, "/r/a", "s1", Status::Idle, Owner::Conductor),
        ]),
        vec![
            own("CC:1", "/r/a", Some("s0")),
            own("CC:2", "/r/a", Some("s1")),
        ],
        Some("CC:2"),
        false,
        at(200),
    );
    let rows = fleet.visual_rows();
    assert_eq!(
        rows[0].key,
        RowKey::Panel("CC:1".into()),
        "選ばれているのは別の行"
    );
    assert_eq!(rows[1].state, State::Idle);
}

#[test]
fn clear_で会話の_id_が変わっても行と選択は変わらない() {
    let mut fleet = FleetPanel::default();
    fleet.apply(
        status(vec![live(
            7,
            "/r/a",
            "old",
            Status::Working,
            Owner::Conductor,
        )]),
        vec![own("CC:1", "/r/a", Some("old")), own("CC:2", "/r/a", None)],
        None,
        false,
        at(200),
    );
    let ws = Workspace::for_test();
    fleet.update(Action::GoToTop, &ws.ctx());
    assert_eq!(
        fleet.selected_row().unwrap().key,
        RowKey::Panel("CC:1".into())
    );
    // 状態ファイルは先に新しい id になり、窓の側はまだ古い id を持っている瞬間。
    fleet.apply(
        status(vec![live(
            7,
            "/r/a",
            "new",
            Status::Working,
            Owner::Conductor,
        )]),
        vec![own("CC:1", "/r/a", Some("old")), own("CC:2", "/r/a", None)],
        None,
        false,
        at(201),
    );
    let selected = fleet.selected_row().unwrap();
    assert_eq!(selected.key, RowKey::Panel("CC:1".into()));
    assert_eq!(selected.state, State::Unknown);
}

#[test]
fn 知らない状態はそのまま出す() {
    let mut fleet = FleetPanel::default();
    fleet.apply(
        status(vec![live(
            7,
            "/r/a",
            "s1",
            Status::Other("shell".into()),
            Owner::Conductor,
        )]),
        Vec::new(),
        None,
        false,
        at(200),
    );
    assert_eq!(fleet.visual_rows()[0].state, State::Other("shell".into()));
}

#[test]
fn 題は会話の_id_で引き_無ければ空() {
    let mut fleet = FleetPanel::default();
    fleet.apply(
        FleetStatus {
            live: vec![
                live(7, "/r/a", "s1", Status::Working, Owner::Conductor),
                live(8, "/r/a", "s2", Status::Working, Owner::Conductor),
            ],
            summaries: vec![(
                "s1".into(),
                at(10),
                Summary {
                    title: Title::Ai("並列Claude監視用の新しいConductor UI".into()),
                    context_tokens: Some(312_000),
                },
            )],
        },
        Vec::new(),
        None,
        false,
        at(200),
    );
    let rows = fleet.visual_rows();
    assert_eq!(
        rows[0].title.as_deref(),
        Some("並列Claude監視用の新しいConductor UI")
    );
    assert_eq!(rows[0].context_tokens, Some(312_000));
    assert_eq!(rows[1].title, None);
    assert_eq!(fleet.known_transcripts().get("s1"), Some(&at(10)));
}

#[test]
fn enter_は選んだ行に入り_esc_は入る前へ戻る() {
    let mut fleet = FleetPanel::default();
    fleet.open(Focus::Viewer, vec![own("CC:1", "/r/a", None)]);
    let ws = Workspace::for_test();
    assert!(matches!(
        fleet.update(Action::Select, &ws.ctx()).as_deref(),
        Some([Effect::EnterSession(RowKey::Panel(id))]) if id == "CC:1"
    ));
    assert!(matches!(
        fleet.update(Action::ExitSubPanel, &ws.ctx()).as_deref(),
        Some([Effect::LeaveFleet(Focus::Viewer)])
    ));
}

#[test]
fn 一覧の操作でないものは拾わない() {
    let mut fleet = FleetPanel::default();
    let ws = Workspace::for_test();
    assert!(fleet.update(Action::DeleteWorktree, &ws.ctx()).is_none());
}

#[test]
fn 重ねた入力待ちの行を選んだまま入力待ちが終わっても同じセッションに留まる() {
    let mut fleet = FleetPanel::default();
    let with = |s7: Status| {
        status(vec![
            live(6, "/r/a", "s0", Status::Working, Owner::Conductor),
            live(7, "/r/b", "s1", s7, Owner::Conductor),
        ])
    };
    fleet.apply(
        with(Status::Waiting(Waiting::Permission)),
        Vec::new(),
        None,
        false,
        at(200),
    );
    let selected = |fleet: &FleetPanel| fleet.selected_row().map(|r| (r.key, r.pinned));
    assert_eq!(selected(&fleet), Some((RowKey::Pid(7), true)));
    fleet.apply(with(Status::Working), Vec::new(), None, false, at(201));
    assert_eq!(selected(&fleet), Some((RowKey::Pid(7), false)));
}

#[test]
fn 会話の_id_が外れても同じ_cwd_に1本ずつなら1行にまとまる() {
    let mut fleet = FleetPanel::default();
    fleet.apply(
        status(vec![live(
            7,
            "/r/a",
            "new",
            Status::Working,
            Owner::Conductor,
        )]),
        vec![own("CC:1", "/r/a", Some("old"))],
        None,
        false,
        at(200),
    );
    let rows = fleet.visual_rows();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].key, RowKey::Panel("CC:1".into()));
    assert_eq!(rows[0].state, State::Working);
}

#[test]
fn 会話の_id_が外れたものが同じ_cwd_に2本あれば合わせない() {
    let mut fleet = FleetPanel::default();
    fleet.apply(
        status(vec![
            live(7, "/r/a", "new1", Status::Working, Owner::Conductor),
            live(8, "/r/a", "new2", Status::Idle, Owner::Conductor),
        ]),
        vec![
            own("CC:1", "/r/a", Some("old1")),
            own("CC:2", "/r/a", Some("old2")),
        ],
        None,
        false,
        at(200),
    );
    assert_eq!(fleet.visual_rows().len(), 4);
}

#[test]
fn 一覧で選んだまま止まった行は待機になる() {
    let mut fleet = FleetPanel::default();
    let one = |s: Status, since: u64| {
        let mut l = live(7, "/r/a", "s1", s, Owner::Conductor);
        l.status_since = Some(at(since));
        status(vec![l])
    };
    fleet.apply(one(Status::Working, 100), Vec::new(), None, true, at(150));
    fleet.apply(one(Status::Idle, 300), Vec::new(), None, true, at(301));
    assert_eq!(fleet.selected_row().unwrap().state, State::Idle);
}

fn ask_stop(fleet: &mut FleetPanel) -> crate::modal::Confirm {
    let ws = Workspace::for_test();
    let mut effects = fleet.update(Action::StopSession, &ws.ctx()).unwrap();
    match effects.pop() {
        Some(Effect::PushModal(crate::modal::Modal::Confirm(confirm))) if effects.is_empty() => {
            confirm
        }
        other => panic!("{other:?} {effects:?}"),
    }
}

#[test]
fn この窓の行で止めるキーを押すとそのパネルを止める確認が出る() {
    let mut fleet = FleetPanel::default();
    fleet.apply(
        FleetStatus {
            live: vec![live(7, "/r/a", "s1", Status::Working, Owner::Conductor)],
            summaries: vec![(
                "s1".into(),
                at(10),
                Summary {
                    title: Title::Ai("捨てレポジトリを片付ける".into()),
                    context_tokens: None,
                },
            )],
        },
        vec![own("CC:1", "/r/a", Some("s1"))],
        None,
        false,
        at(200),
    );
    let confirm = ask_stop(&mut fleet);
    assert_eq!(confirm.title, "Stop session");
    assert_eq!(confirm.question, "「捨てレポジトリを片付ける」を止める?");
    assert!(
        matches!(
            confirm.on_yes.as_slice(),
            [Effect::StopFleetSession(StopTarget::Panel(id))] if id == "CC:1"
        ),
        "{:?}",
        confirm.on_yes
    );
}

#[test]
fn この窓の外の行は一覧に載せたときの起動時刻ごと止める相手にする() {
    let mut fleet = FleetPanel::default();
    fleet.apply(
        status(vec![live(9, "/r/a", "s9", Status::Idle, Owner::Elsewhere)]),
        Vec::new(),
        None,
        false,
        at(200),
    );
    let confirm = ask_stop(&mut fleet);
    assert_eq!(confirm.question, "「(まだ何も頼んでいない)」を止める?");
    assert!(
        matches!(
            confirm.on_yes.as_slice(),
            [Effect::StopFleetSession(StopTarget::Process { pid: 9, started })] if *started == at(9)
        ),
        "{:?}",
        confirm.on_yes
    );
}

#[test]
fn 確認を出した後に並びが変わっても止める相手は押した時の行() {
    let mut fleet = FleetPanel::default();
    fleet.apply(
        status(vec![
            live(7, "/r/a", "s1", Status::Working, Owner::Conductor),
            live(8, "/r/b", "s2", Status::Working, Owner::Conductor),
        ]),
        Vec::new(),
        None,
        false,
        at(200),
    );
    assert_eq!(fleet.selected_row().unwrap().key, RowKey::Pid(7));
    let confirm = ask_stop(&mut fleet);
    fleet.apply(
        status(vec![live(
            8,
            "/r/b",
            "s2",
            Status::Waiting(Waiting::Question),
            Owner::Conductor,
        )]),
        Vec::new(),
        None,
        false,
        at(201),
    );
    assert!(matches!(
        confirm.on_yes.as_slice(),
        [Effect::StopFleetSession(StopTarget::Process { pid: 7, .. })]
    ));
}

#[test]
fn 入力待ちの欄の行でも止める相手は同じセッション() {
    let mut fleet = FleetPanel::default();
    fleet.apply(
        status(vec![live(
            7,
            "/r/a",
            "s1",
            Status::Waiting(Waiting::Question),
            Owner::Conductor,
        )]),
        Vec::new(),
        None,
        false,
        at(200),
    );
    assert!(fleet.selected_row().unwrap().pinned);
    let confirm = ask_stop(&mut fleet);
    assert!(matches!(
        confirm.on_yes.as_slice(),
        [Effect::StopFleetSession(StopTarget::Process { pid: 7, started })] if *started == at(7)
    ));
}

#[test]
fn 行が無ければ止める確認は出ない() {
    let mut fleet = FleetPanel::default();
    let ws = Workspace::for_test();
    assert!(
        fleet
            .update(Action::StopSession, &ws.ctx())
            .is_some_and(|effects| effects.is_empty())
    );
}

#[test]
fn 止める確認には場所とディレクトリの末尾と状態を出し_作業中なら途中で止まると書く() {
    const DEEP: &str = "/home/u/ghq/acme/app-worktrees/fix-login";
    let mut fleet = FleetPanel::default();
    fleet.apply(
        status(vec![
            live(7, DEEP, "s1", Status::Working, Owner::Conductor),
            live(8, "/r/b", "s2", Status::Idle, Owner::Conductor),
            live(9, "/c", "s3", Status::Idle, Owner::Elsewhere),
        ]),
        vec![own("CC:1", DEEP, Some("s1"))],
        None,
        true,
        at(200),
    );
    let detail = |key: RowKey| {
        let row = fleet
            .visual_rows()
            .into_iter()
            .find(|r| r.key == key)
            .unwrap();
        render::stop_detail(&row)
    };
    assert_eq!(
        detail(RowKey::Panel("CC:1".into())),
        [
            "場所: この窓",
            "…/app-worktrees/fix-login",
            "状態: 作業中。作業の途中で止まります"
        ]
    );
    assert_eq!(
        detail(RowKey::Pid(8)),
        [
            "場所: conductor の中 (pid 8)",
            "…/r/b",
            "状態: 完了・未確認"
        ]
    );
    assert_eq!(
        detail(RowKey::Pid(9)),
        ["場所: この窓の外 (pid 9)", "/c", "状態: 完了・未確認"]
    );
}

#[test]
fn 止めたこの窓の行は組んでいた写しごとすぐ消え_選択は残りへ移る() {
    let mut fleet = FleetPanel::default();
    fleet.apply(
        status(vec![
            live(7, "/r/a", "s1", Status::Working, Owner::Conductor),
            live(8, "/r/a", "s2", Status::Idle, Owner::Conductor),
        ]),
        vec![
            own("CC:1", "/r/a", Some("s1")),
            own("CC:2", "/r/a", Some("s2")),
        ],
        None,
        false,
        at(200),
    );
    assert_eq!(
        fleet.selected_row().unwrap().key,
        RowKey::Panel("CC:1".into())
    );
    fleet.forget_stopped("CC:1", vec![own("CC:2", "/r/a", Some("s2"))]);
    assert_eq!(keys(&fleet), [(RowKey::Panel("CC:2".into()), false)]);
    assert_eq!(
        fleet.selected_row().unwrap().key,
        RowKey::Panel("CC:2".into())
    );
}

#[test]
fn clear_の後に止めたこの窓の行も_cwd_で組んだ写しごと消える() {
    let mut fleet = FleetPanel::default();
    fleet.apply(
        status(vec![live(
            7,
            "/r/a",
            "new",
            Status::Working,
            Owner::Conductor,
        )]),
        vec![own("CC:1", "/r/a", Some("old"))],
        None,
        false,
        at(200),
    );
    fleet.forget_stopped("CC:1", Vec::new());
    assert!(fleet.visual_rows().is_empty(), "{:?}", fleet.visual_rows());
}

#[test]
fn 外のプロセスを止めた結果はそれぞれの文で帯に出る() {
    use crate::workspace::StatusLevel;
    use conductor_core::claude_sessions::registry::Stopped;
    let mut ws = Workspace::for_test();
    let mut status_of = |stopped| match ws
        .accept(crate::task::TaskResult::SessionStopped(stopped))
        .as_slice()
    {
        [Effect::Status(level, text)] => (*level, text.clone()),
        other => panic!("{other:?}"),
    };
    assert_eq!(
        status_of(Stopped::Terminated),
        (StatusLevel::Success, "Stopped the session.".into())
    );
    assert_eq!(
        status_of(Stopped::Killed),
        (
            StatusLevel::Success,
            "Stopped the session (it ignored SIGTERM, so it was killed).".into()
        )
    );
    assert_eq!(
        status_of(Stopped::AlreadyGone),
        (StatusLevel::Info, "That session had already ended.".into())
    );
    assert_eq!(
        status_of(Stopped::Failed("pid 9 survived SIGKILL".into())),
        (
            StatusLevel::Error,
            "Could not stop the session: pid 9 survived SIGKILL".into()
        )
    );
}

mod hint {
    use conductor_core::keymap::KeyMap;
    use conductor_svc::Services;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use unicode_width::UnicodeWidthStr;

    use super::*;
    use crate::effect::apply;

    fn listing(live: Vec<LiveSession>, own: Vec<OwnSession>) -> Workspace {
        let mut ws = Workspace::for_test();
        let mut svc = Services::<crate::task::TaskResult>::new();
        ws.focus = Focus::Viewer;
        apply(&mut ws, &mut svc, vec![Effect::OpenFleet]);
        ws.apply_fleet(status(live), own);
        ws
    }

    fn rebind(ws: &mut Workspace, fleet_layer: &str) {
        let user: toml::Table = toml::from_str(&format!("[layers.fleet]\n{fleet_layer}")).unwrap();
        ws.keymap = KeyMap::with_warnings(&user).0;
    }

    fn drawn(ws: &Workspace, width: u16, height: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| render::list(frame, frame.area(), ws))
            .unwrap();
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                let mut line = String::new();
                let mut x = 0;
                while x < width {
                    let symbol = buffer[(x, y)].symbol();
                    line.push_str(symbol);
                    x += u16::try_from(symbol.width().max(1)).unwrap();
                }
                line
            })
            .collect()
    }

    #[test]
    fn この窓の行では開く_返事_止める_閉じるを出す() {
        let ws = listing(Vec::new(), vec![own("CC:1", "/r/a", None)]);
        assert_eq!(
            render::hint(&ws),
            "enter 開く · tab 返事 · D 止める · esc 閉じる"
        );
    }

    #[test]
    fn この窓の外の行では止めると閉じるだけ() {
        let ws = listing(
            vec![live(9, "/r/a", "s9", Status::Idle, Owner::Elsewhere)],
            Vec::new(),
        );
        assert_eq!(render::hint(&ws), "D 止める · esc 閉じる");
    }

    #[test]
    fn 行が無ければ閉じるだけ() {
        let ws = listing(Vec::new(), Vec::new());
        assert_eq!(render::hint(&ws), "esc 閉じる");
    }

    #[test]
    fn 右に入っている間は戻るキーと左右が_claude_に届くことを出す() {
        let mut ws = listing(Vec::new(), vec![own("CC:1", "/r/a", None)]);
        ws.focus = Focus::FleetSession;
        assert_eq!(
            render::hint(&ws),
            "ctrl+esc 一覧へ戻る · ← などは Claude に届く"
        );
    }

    #[test]
    fn 止めるキーを外せば案内から消え_押しても何も起きない() {
        let mut ws = listing(Vec::new(), vec![own("CC:1", "/r/a", None)]);
        rebind(&mut ws, "\"D\" = false\n");
        assert_eq!(render::hint(&ws), "enter 開く · tab 返事 · esc 閉じる");
        let key = crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('D'),
            crossterm::event::KeyModifiers::SHIFT,
        );
        assert_eq!(
            crate::route::route(&mut ws, key),
            crate::route::Routed::Ignored
        );
    }

    #[test]
    fn 止めるキーを付け替えれば案内もそのキーになる() {
        let mut ws = listing(
            vec![live(9, "/r/a", "s9", Status::Idle, Owner::Elsewhere)],
            Vec::new(),
        );
        rebind(&mut ws, "\"D\" = false\n\"x\" = \"stop_session\"\n");
        assert_eq!(render::hint(&ws), "x 止める · esc 閉じる");
    }

    #[test]
    fn 案内は一覧の最下行に出し_幅が足りなければ折り返さずに切る() {
        let ws = listing(Vec::new(), vec![own("CC:1", "/r/a", None)]);
        let lines = drawn(&ws, 24, 8);
        let footer = lines[6].trim_end();
        assert!(footer.starts_with(" enter 開く"), "{lines:#?}");
        assert!(footer.ends_with('…'), "{lines:#?}");
        assert!(!lines[5].contains("閉じる"), "{lines:#?}");
    }

    #[test]
    fn 一覧が低くても落ちず_1_行なら案内だけ() {
        let ws = listing(
            vec![live(9, "/r/a", "s9", Status::Idle, Owner::Elsewhere)],
            Vec::new(),
        );
        for height in 0..=4 {
            drawn(&ws, 60, height);
        }
        let lines = drawn(&ws, 60, 3);
        assert_eq!(lines[1].trim(), "D 止める · esc 閉じる");
    }

    #[test]
    fn 下の方の行を選んでも案内に隠れない() {
        let mut ws = listing(Vec::new(), Vec::new());
        ws.apply_fleet(
            FleetStatus {
                live: (1..=20)
                    .map(|pid| {
                        let session = format!("s{pid}");
                        live(pid, "/r/a", &session, Status::Idle, Owner::Elsewhere)
                    })
                    .collect(),
                summaries: (1..=20)
                    .map(|pid| {
                        let title = Title::Ai(format!("row-{pid}"));
                        let summary = Summary {
                            title,
                            context_tokens: None,
                        };
                        (format!("s{pid}"), at(0), summary)
                    })
                    .collect(),
            },
            Vec::new(),
        );
        let other = Workspace::for_test();
        ws.panels.fleet.update(Action::GoToBottom, &other.ctx());
        assert_eq!(ws.panels.fleet.selected_row().unwrap().key, RowKey::Pid(20));
        let lines = drawn(&ws, 60, 12);
        assert!(lines[10].contains("止める"), "{lines:#?}");
        assert!(
            lines[1..10].iter().any(|l| l.contains("row-20")),
            "{lines:#?}"
        );
    }
}

mod screen {
    use conductor_core::cc_hook::UiRequest;
    use conductor_svc::Services;

    use super::*;
    use crate::command::CommandId;
    use crate::effect::apply;
    use crate::workspace::StatusLevel;
    use conductor_svc::pty::SessionKind;

    fn opened() -> (Workspace, Services<crate::task::TaskResult>) {
        let mut ws = Workspace::for_test();
        let mut svc = Services::new();
        ws.focus = Focus::Viewer;
        apply(&mut ws, &mut svc, vec![Effect::OpenFleet]);
        assert!(ws.chrome.fleet_open);
        assert_eq!(ws.focus, Focus::Fleet);
        (ws, svc)
    }

    #[test]
    fn 一覧の上では今の画面へのフォーカス移動を出たときの行き先にする() {
        let (mut ws, mut svc) = opened();
        apply(&mut ws, &mut svc, vec![Effect::Focus(Focus::Explorer)]);
        assert!(ws.chrome.fleet_open);
        assert_eq!(ws.focus, Focus::Fleet);
        assert_eq!(ws.panels.fleet.came_from(), Focus::Explorer);
    }

    #[test]
    fn 一覧の上で起こしたセッションへはフォーカスが移らない() {
        let (mut ws, mut svc) = opened();
        let dir = tempfile::tempdir().unwrap();
        ws.config.general.shell = "/bin/sh".into();
        ws.repo.root = dir.path().to_path_buf();
        apply(
            &mut ws,
            &mut svc,
            vec![Effect::NewSession(SessionKind::Shell)],
        );
        assert!(ws.panels.terminal.has_shell());
        assert!(ws.chrome.fleet_open);
        assert_eq!(ws.focus, Focus::Fleet);
        assert_eq!(ws.panels.fleet.came_from(), Focus::TerminalShell);
    }

    #[test]
    fn 一覧の上でもフォーカスのコマンドは一覧を閉じてそこへ移る() {
        let (mut ws, mut svc) = opened();
        let effects = crate::command::execute(&mut ws, CommandId::FocusTerminalShell);
        apply(&mut ws, &mut svc, effects);
        assert!(!ws.chrome.fleet_open);
        assert_eq!(ws.focus, Focus::TerminalShell);
    }

    #[test]
    fn esc_は一覧に入る前のフォーカスへ戻る() {
        let (mut ws, mut svc) = opened();
        let effects = ws.dispatch(Action::ExitSubPanel).unwrap();
        apply(&mut ws, &mut svc, effects);
        assert!(!ws.chrome.fleet_open);
        assert_eq!(ws.focus, Focus::Viewer);
    }

    #[test]
    fn 一覧の上では_claude_からの画面操作で画面が動かない() {
        let repo = crate::testing::TestRepo::new();
        let feat = repo.worktree("feat");
        let (mut ws, mut svc) = crate::testing::workspace_for(&repo);
        apply(&mut ws, &mut svc, vec![Effect::OpenFleet]);
        let shown = ws.worktree_path();
        assert_ne!(shown, feat);
        let effects = crate::remote::effects(&mut ws, &feat, UiRequest::AnalyzeReview);
        assert!(
            matches!(
                effects.as_slice(),
                [Effect::Status(StatusLevel::Warning, _)]
            ),
            "{effects:?}"
        );
        apply(&mut ws, &mut svc, effects);
        assert!(ws.chrome.fleet_open);
        assert!(ws.modals.is_empty());
        assert_eq!(ws.worktree_path(), shown);
    }

    #[test]
    fn この窓の外の行に_enter_しても入らない() {
        let (mut ws, mut svc) = opened();
        apply(
            &mut ws,
            &mut svc,
            vec![Effect::EnterSession(RowKey::Pid(9))],
        );
        assert!(ws.chrome.fleet_open);
        assert_eq!(ws.focus, Focus::Fleet);
        assert!(matches!(
            ws.chrome.status.as_ref().map(|s| s.level),
            Some(StatusLevel::Warning)
        ));
    }

    #[test]
    fn レビューの_claude_列から一覧を開いて_esc_するとレビューに戻る() {
        let mut ws = Workspace::for_test();
        let mut svc = Services::new();
        ws.focus = Focus::TerminalClaude;
        ws.chrome.review_open = true;
        apply(&mut ws, &mut svc, vec![Effect::OpenFleet]);
        let effects = ws.dispatch(Action::ExitSubPanel).unwrap();
        apply(&mut ws, &mut svc, effects);
        assert_eq!(ws.focus, Focus::TerminalClaude);
        assert!(ws.reviewing());
    }

    #[test]
    fn 下がレビュー画面の一覧でレビューのコマンドを押すとレビューを閉じて_explorer_へ() {
        let mut ws = Workspace::for_test();
        let mut svc = Services::new();
        ws.focus = Focus::Revidere;
        ws.chrome.review_open = true;
        apply(&mut ws, &mut svc, vec![Effect::OpenFleet]);
        apply(
            &mut ws,
            &mut svc,
            vec![Effect::Command(crate::command::CommandId::ShowRevidere)],
        );
        assert!(!ws.chrome.fleet_open);
        assert_eq!(ws.focus, Focus::Explorer);
        assert!(!ws.reviewing());
    }

    #[test]
    fn 一覧の上のフォーカスの輪は一覧を閉じる() {
        let mut ws = Workspace::for_test();
        let mut svc = Services::new();
        ws.focus = Focus::TerminalClaude;
        apply(&mut ws, &mut svc, vec![Effect::OpenFleet]);
        let effects = crate::route::global_effects(&ws, Action::CycleFocusForward);
        apply(&mut ws, &mut svc, effects);
        assert!(!ws.chrome.fleet_open);
        assert_eq!(ws.focus, Focus::TerminalClaude.next());
    }

    #[test]
    fn 一覧を閉じている間の選び直しは見たことにしない() {
        let mut ws = Workspace::for_test();
        let mut svc = Services::new();
        ws.focus = Focus::Viewer;
        let working = live(8, "/r/b", "s2", Status::Working, Owner::Conductor);
        ws.accept(crate::task::TaskResult::FleetStatus(Box::new(status(
            vec![working],
        ))));
        apply(&mut ws, &mut svc, vec![Effect::OpenFleet]);
        let effects = ws.dispatch(Action::ExitSubPanel).unwrap();
        apply(&mut ws, &mut svc, effects);
        let stopped = live(7, "/r/a", "s1", Status::Idle, Owner::Conductor);
        ws.accept(crate::task::TaskResult::FleetStatus(Box::new(status(
            vec![stopped],
        ))));
        let row = ws.panels.fleet.selected_row().unwrap();
        assert_eq!(row.key, RowKey::Pid(7));
        assert_eq!(row.state, State::Finished);
    }

    #[test]
    fn 一覧の上のファイルのドロップは見えない_claude_に入れない() {
        let (mut ws, mut svc) = opened();
        let dir = tempfile::tempdir().unwrap();
        ws.panels.terminal.show_shell_as_claude_for_test(dir.path());
        let file = dir.path().join("a.txt");
        std::fs::write(&file, "x").unwrap();
        crate::run::on_paste(&mut ws, &mut svc, format!("{} ", file.display()));
        assert!(ws.chrome.fleet_open);
        assert!(matches!(
            ws.chrome.status.as_ref().map(|s| s.level),
            Some(StatusLevel::Warning)
        ));
    }

    #[test]
    fn 一覧に載っている別の_worktree_のセッションに_enter_するとその_worktree_に入る() {
        let repo = crate::testing::TestRepo::new();
        let feat = repo.worktree("feat");
        let (mut ws, mut svc) = crate::testing::workspace_for(&repo);
        crate::testing::pump(&mut ws, &mut svc);
        ws.panels.terminal.show_shell_as_claude_for_test(&feat);
        let id = ws.panels.terminal.visible_claude().unwrap().to_string();
        apply(&mut ws, &mut svc, vec![Effect::OpenFleet]);
        apply(
            &mut ws,
            &mut svc,
            vec![Effect::EnterSession(RowKey::Panel(id))],
        );
        assert!(!ws.chrome.fleet_open);
        assert_eq!(
            std::fs::canonicalize(ws.worktree_path()).unwrap(),
            std::fs::canonicalize(&feat).unwrap()
        );
        assert_eq!(ws.focus, Focus::TerminalClaude);
    }

    #[test]
    fn 一覧に載る前の同じ_repo_の_worktree_には入らず_repo_も解析もそのまま() {
        let repo = crate::testing::TestRepo::new();
        let (mut ws, mut svc) = crate::testing::workspace_for(&repo);
        apply(&mut ws, &mut svc, vec![Effect::OpenFleet]);
        let feat = repo.worktree("feat");
        ws.panels.terminal.show_shell_as_claude_for_test(&feat);
        let id = ws.panels.terminal.visible_claude().unwrap().to_string();
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        ws.panels
            .revidere
            .note_spawned(&crate::task::Task::Analyze {
                worktree: repo.root(),
                branch: "main".into(),
                scope: Default::default(),
                force: false,
                api: Default::default(),
                review: Default::default(),
                cancel: cancel.clone(),
            });
        let before = svc.generation();
        apply(
            &mut ws,
            &mut svc,
            vec![Effect::EnterSession(RowKey::Panel(id))],
        );
        assert_eq!(svc.generation(), before, "同じ repo を開き直した");
        assert!(!cancel.load(std::sync::atomic::Ordering::Relaxed));
        assert!(ws.chrome.fleet_open);
        assert!(matches!(
            ws.chrome.status.as_ref().map(|s| s.level),
            Some(StatusLevel::Warning)
        ));
    }

    fn enter_from_review(
        a: &crate::testing::TestRepo,
        dir: impl FnOnce(&mut Workspace) -> std::path::PathBuf,
    ) -> (
        Workspace,
        Services<crate::task::TaskResult>,
        std::path::PathBuf,
    ) {
        let (mut ws, mut svc) = crate::testing::workspace_for(a);
        crate::testing::pump(&mut ws, &mut svc);
        let dir = dir(&mut ws);
        ws.panels.terminal.show_shell_as_claude_for_test(&dir);
        let id = ws.panels.terminal.visible_claude().unwrap().to_string();
        ws.focus = Focus::Revidere;
        ws.chrome.review_open = true;
        apply(&mut ws, &mut svc, vec![Effect::OpenFleet]);
        apply(
            &mut ws,
            &mut svc,
            vec![Effect::EnterSession(RowKey::Panel(id))],
        );
        crate::testing::pump(&mut ws, &mut svc);
        (ws, svc, dir)
    }

    fn same_dir(a: &std::path::Path, b: &std::path::Path) -> bool {
        std::fs::canonicalize(a).unwrap() == std::fs::canonicalize(b).unwrap()
    }

    #[test]
    fn レビュー画面から同じ_worktree_のセッションに入るとレビュー画面のまま() {
        let a = crate::testing::TestRepo::new();
        let (ws, _svc, _) = enter_from_review(&a, |ws| ws.worktree_path());
        assert!(!ws.chrome.fleet_open);
        assert!(ws.reviewing());
        assert_eq!(ws.focus, Focus::TerminalClaude);
    }

    #[test]
    fn レビュー画面から同じ_repo_の別の_worktree_に入るとworktree_の切り替えと同じくレビュー画面を閉じる()
     {
        let a = crate::testing::TestRepo::new();
        let feat = a.worktree("feat");
        let (ws, _svc, _) = enter_from_review(&a, |_| feat.clone());
        assert!(same_dir(&ws.worktree_path(), &feat));
        assert!(!ws.reviewing());
        assert_eq!(ws.focus, Focus::TerminalClaude);
    }

    #[test]
    fn レビュー画面から別の_repo_の_main_worktree_に入るとレビュー画面を閉じる() {
        let a = crate::testing::TestRepo::new();
        let b = crate::testing::TestRepo::new();
        let (ws, _svc, _) = enter_from_review(&a, |_| b.root());
        assert!(same_dir(&ws.worktree_path(), &b.root()));
        assert!(!ws.reviewing());
        assert_eq!(ws.focus, Focus::TerminalClaude);
    }

    #[test]
    fn レビュー画面から別の_repo_の_linked_worktree_に入ると一覧が届いた後もそのセッションにいる() {
        let a = crate::testing::TestRepo::new();
        let b = crate::testing::TestRepo::new();
        let b_feat = b.worktree("feat");
        let (ws, _svc, _) = enter_from_review(&a, |_| b_feat.clone());
        assert!(same_dir(&ws.worktree_path(), &b_feat));
        assert!(!ws.reviewing());
        assert_eq!(ws.focus, Focus::TerminalClaude);
    }

    #[test]
    fn 別の_repo_から入った_linked_worktree_に預けたレビュー画面があれば一覧が届いた時に戻る() {
        let a = crate::testing::TestRepo::new();
        let b = crate::testing::TestRepo::new();
        let b_feat = b.worktree("feat");
        let (ws, _svc, _) = enter_from_review(&a, |ws| {
            ws.chrome
                .review_open_in
                .insert(b_feat.clone(), Focus::TerminalClaude);
            b_feat.clone()
        });
        assert!(same_dir(&ws.worktree_path(), &b_feat));
        assert!(ws.reviewing());
        assert_eq!(ws.focus, Focus::TerminalClaude);
    }

    #[test]
    fn エディタを開いていれば別の_worktree_のセッションには入らない() {
        let repo = crate::testing::TestRepo::new();
        let feat = repo.worktree("feat");
        let (mut ws, mut svc) = crate::testing::workspace_for(&repo);
        crate::testing::pump(&mut ws, &mut svc);
        let root = ws.worktree_path();
        ws.panels
            .terminal
            .open_editor(&root.join("a.txt"), &root, &["/bin/sh".to_string()])
            .unwrap();
        ws.panels.terminal.show_shell_as_claude_for_test(&feat);
        let id = ws.panels.terminal.visible_claude().unwrap().to_string();
        apply(&mut ws, &mut svc, vec![Effect::OpenFleet]);
        apply(
            &mut ws,
            &mut svc,
            vec![Effect::EnterSession(RowKey::Panel(id))],
        );
        assert!(ws.chrome.fleet_open);
        assert_eq!(ws.worktree_path(), root);
        assert!(ws.panels.terminal.editor_name().is_some());
    }
}

mod pane {
    use std::path::Path;

    use conductor_svc::Services;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::layout::Rect;

    use super::*;
    use crate::effect::apply;
    use crate::layout::Region;
    use crate::panels::terminal::render::{content_area, editor_area};
    use crate::run::{on_key, on_paste};

    type Svc = Services<crate::task::TaskResult>;

    fn opened() -> (Workspace, Svc, tempfile::TempDir) {
        let mut ws = Workspace::for_test();
        let mut svc = Services::new();
        ws.focus = Focus::Viewer;
        apply(&mut ws, &mut svc, vec![Effect::OpenFleet]);
        (ws, svc, tempfile::tempdir().unwrap())
    }

    /// 中身はシェル。Claude 区画に映すが、一覧の外で大きさを合わせるのはシェル区画。
    fn spawn(ws: &mut Workspace, dir: &Path) -> String {
        ws.panels.terminal.show_shell_as_claude_for_test(dir);
        ws.panels.terminal.visible_claude().unwrap().to_string()
    }

    fn list(ws: &mut Workspace, dir: &Path, ids: &[&str]) {
        let cwd = dir.to_string_lossy();
        let own = ids.iter().map(|id| own(id, &cwd, None)).collect();
        ws.apply_fleet(FleetStatus::default(), own);
    }

    fn press(ws: &mut Workspace, svc: &mut Svc, code: KeyCode, modifiers: KeyModifiers) {
        on_key(ws, svc, KeyEvent::new(code, modifiers));
    }

    fn shows(ws: &Workspace, id: &str, text: &str) -> bool {
        ws.panels
            .terminal
            .screen_for_test(id)
            .contents()
            .contains(text)
    }

    fn size_of(ws: &Workspace, id: &str) -> (u16, u16) {
        ws.panels.terminal.screen_for_test(id).size()
    }

    fn grid_after_layout(ws: &mut Workspace, area: Rect, region: Region) -> (u16, u16) {
        let layout = crate::layout::layout(ws, area);
        ws.sync_layout(&layout);
        let rect = layout.rect(region).unwrap();
        let grid = match region {
            Region::FleetSession => editor_area(rect),
            _ => content_area(rect),
        };
        (grid.height, grid.width)
    }

    #[test]
    fn tab_で右に入ると打鍵は一覧で選んだセッションへ行く() {
        let (mut ws, mut svc, dir) = opened();
        let listed = spawn(&mut ws, dir.path());
        let hidden = spawn(&mut ws, dir.path());
        list(&mut ws, dir.path(), &[&listed]);
        press(&mut ws, &mut svc, KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(ws.focus, Focus::FleetSession);
        for c in "echo typed-$((40+2))".chars() {
            press(&mut ws, &mut svc, KeyCode::Char(c), KeyModifiers::NONE);
        }
        press(&mut ws, &mut svc, KeyCode::Enter, KeyModifiers::NONE);
        crate::testing::wait_for(|| shows(&ws, &listed, "typed-42"));
        assert!(!shows(&ws, &hidden, "typed"));
    }

    #[test]
    fn 右に入っていれば日本語の貼り付けもファイルのドロップもそのセッションへ() {
        let (mut ws, mut svc, dir) = opened();
        let listed = spawn(&mut ws, dir.path());
        let hidden = spawn(&mut ws, dir.path());
        list(&mut ws, dir.path(), &[&listed]);
        apply(&mut ws, &mut svc, vec![Effect::Focus(Focus::FleetSession)]);
        let file = dir.path().join("dropped.txt");
        std::fs::write(&file, "x").unwrap();
        on_paste(&mut ws, &mut svc, "日本語".into());
        on_paste(&mut ws, &mut svc, format!("{} ", file.display()));
        crate::testing::wait_for(|| {
            shows(&ws, &listed, "日本語") && shows(&ws, &listed, "dropped.txt")
        });
        assert!(ws.chrome.status.is_none(), "{:?}", ws.chrome.status);
        assert_eq!(ws.focus, Focus::FleetSession);
        assert!(!shows(&ws, &hidden, "dropped.txt"));
    }

    #[test]
    fn 右の区画にはセッションの画面が描かれる() {
        let (mut ws, mut svc, dir) = opened();
        let listed = spawn(&mut ws, dir.path());
        list(&mut ws, dir.path(), &[&listed]);
        apply(&mut ws, &mut svc, vec![Effect::Focus(Focus::FleetSession)]);
        on_paste(&mut ws, &mut svc, "echo drawn-$((1+1))\n".into());
        crate::testing::wait_for(|| shows(&ws, &listed, "drawn-2"));
        let backend = ratatui::backend::TestBackend::new(200, 50);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let mut right = Rect::default();
        terminal
            .draw(|frame| {
                let layout = crate::layout::layout(&ws, frame.area());
                right = layout.rect(Region::FleetSession).unwrap();
                crate::render::render(frame, &ws, &layout);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let drawn: String = (right.y..right.bottom())
            .flat_map(|y| (right.x..right.right()).map(move |x| (x, y)))
            .map(|(x, y)| buffer[(x, y)].symbol())
            .collect();
        assert!(drawn.contains("drawn-2"), "{drawn}");
    }

    #[test]
    fn この窓の外の行では_tab_で右に入らない() {
        let (mut ws, mut svc, _dir) = opened();
        let outside = live(9, "/r/a", "s9", Status::Working, Owner::Elsewhere);
        ws.apply_fleet(status(vec![outside]), Vec::new());
        press(&mut ws, &mut svc, KeyCode::Tab, KeyModifiers::NONE);
        assert_eq!(ws.focus, Focus::Fleet);
    }

    #[test]
    fn 右で_ctrl_esc_を押すと一覧の左へ戻る() {
        let (mut ws, mut svc, dir) = opened();
        let listed = spawn(&mut ws, dir.path());
        list(&mut ws, dir.path(), &[&listed]);
        apply(&mut ws, &mut svc, vec![Effect::Focus(Focus::FleetSession)]);
        press(&mut ws, &mut svc, KeyCode::Esc, KeyModifiers::CONTROL);
        assert!(ws.chrome.fleet_open);
        assert_eq!(ws.focus, Focus::Fleet);
    }

    #[test]
    fn 右から一覧を開き直しても_esc_は一覧に入る前の画面へ戻る() {
        let (mut ws, mut svc, dir) = opened();
        let listed = spawn(&mut ws, dir.path());
        list(&mut ws, dir.path(), &[&listed]);
        apply(&mut ws, &mut svc, vec![Effect::Focus(Focus::FleetSession)]);
        press(&mut ws, &mut svc, KeyCode::Char('1'), KeyModifiers::ALT);
        assert_eq!(ws.focus, Focus::Fleet);
        let effects = ws.dispatch(Action::ExitSubPanel).unwrap();
        apply(&mut ws, &mut svc, effects);
        assert!(!ws.chrome.fleet_open);
        assert_eq!(ws.focus, Focus::Viewer);
    }

    #[test]
    fn 打っていたセッションの行が消えたら一覧の左へ戻り選び直した先へは打たない() {
        let (mut ws, mut svc, dir) = opened();
        let a = spawn(&mut ws, dir.path());
        let b = spawn(&mut ws, dir.path());
        list(&mut ws, dir.path(), &[&a, &b]);
        apply(&mut ws, &mut svc, vec![Effect::Focus(Focus::FleetSession)]);
        let typing = ws.fleet_session().unwrap();
        let rest = if typing == a { b } else { a };
        list(&mut ws, dir.path(), &[&rest]);
        assert_eq!(ws.fleet_session(), Some(rest));
        assert_eq!(ws.focus, Focus::Fleet);
    }

    #[test]
    fn 打っていたセッションが片付いた後の一覧で外の行が選ばれたら一覧の左へ戻る() {
        let (mut ws, mut svc, dir) = opened();
        let listed = spawn(&mut ws, dir.path());
        list(&mut ws, dir.path(), &[&listed]);
        apply(&mut ws, &mut svc, vec![Effect::Focus(Focus::FleetSession)]);
        ws.panels.terminal.stop_sessions(None);
        assert_eq!(ws.pty_session(), None);
        let outside = live(9, "/r/a", "s9", Status::Working, Owner::Elsewhere);
        ws.apply_fleet(status(vec![outside]), Vec::new());
        assert_eq!(ws.focus, Focus::Fleet);
    }

    #[test]
    fn 他のセッションの行が消えても右に映すものと打つ先は変わらない() {
        let (mut ws, mut svc, dir) = opened();
        let a = spawn(&mut ws, dir.path());
        let b = spawn(&mut ws, dir.path());
        list(&mut ws, dir.path(), &[&a, &b]);
        apply(&mut ws, &mut svc, vec![Effect::Focus(Focus::FleetSession)]);
        let typing = ws.fleet_session().unwrap();
        list(&mut ws, dir.path(), &[&typing]);
        assert_eq!(ws.fleet_session(), Some(typing));
        assert_eq!(ws.focus, Focus::FleetSession);
    }

    #[test]
    fn 右は区画の大きさに合わせ_一覧を閉じると今の画面の区画の大きさへ戻す() {
        const AREA: Rect = Rect::new(0, 0, 200, 50);
        let mut ws = Workspace::for_test();
        let mut svc = Services::new();
        ws.focus = Focus::Viewer;
        let dir = tempfile::tempdir().unwrap();
        let listed = spawn(&mut ws, dir.path());
        let screen = grid_after_layout(&mut ws, AREA, Region::TerminalShell);
        assert_eq!(size_of(&ws, &listed), screen);

        apply(&mut ws, &mut svc, vec![Effect::OpenFleet]);
        list(&mut ws, dir.path(), &[&listed]);
        let fleet = grid_after_layout(&mut ws, AREA, Region::FleetSession);
        assert_ne!(fleet, screen);
        assert_eq!(size_of(&ws, &listed), fleet);

        let effects = ws.dispatch(Action::ExitSubPanel).unwrap();
        apply(&mut ws, &mut svc, effects);
        grid_after_layout(&mut ws, AREA, Region::TerminalShell);
        assert_eq!(size_of(&ws, &listed), screen);
    }

    #[test]
    fn 端末の窓の大きさが変われば右のセッションも追従する() {
        let (mut ws, _svc, dir) = opened();
        let listed = spawn(&mut ws, dir.path());
        list(&mut ws, dir.path(), &[&listed]);
        grid_after_layout(&mut ws, Rect::new(0, 0, 160, 40), Region::FleetSession);
        let wider = grid_after_layout(&mut ws, Rect::new(0, 0, 220, 60), Region::FleetSession);
        assert_eq!(size_of(&ws, &listed), wider);
    }
}

mod stop {
    use std::path::Path;

    use conductor_svc::Services;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::*;
    use crate::effect::apply;
    use crate::route::{Routed, route};
    use crate::run::{on_key, on_paste};
    use crate::workspace::StatusLevel;

    type Svc = Services<crate::task::TaskResult>;

    fn opened() -> (Workspace, Svc) {
        let mut ws = Workspace::for_test();
        let mut svc = Services::new();
        ws.focus = Focus::Viewer;
        apply(&mut ws, &mut svc, vec![Effect::OpenFleet]);
        (ws, svc)
    }

    fn spawn(ws: &mut Workspace, dir: &Path) -> String {
        ws.panels.terminal.show_shell_as_claude_for_test(dir);
        ws.panels.terminal.visible_claude().unwrap().to_string()
    }

    fn list(ws: &mut Workspace, dir: &Path, ids: &[&String]) {
        let cwd = dir.to_string_lossy();
        let own = ids.iter().map(|id| own(id, &cwd, None)).collect();
        ws.apply_fleet(FleetStatus::default(), own);
    }

    fn type_text(ws: &mut Workspace, svc: &mut Svc, text: &str) {
        for c in text.chars() {
            let modifiers = if c.is_ascii_uppercase() {
                KeyModifiers::SHIFT
            } else {
                KeyModifiers::NONE
            };
            on_key(ws, svc, KeyEvent::new(KeyCode::Char(c), modifiers));
        }
    }

    fn present(ws: &Workspace, id: &str) -> bool {
        ws.panels.terminal.session_dir(id).is_some()
    }

    fn status_of(ws: &Workspace) -> Option<(StatusLevel, String)> {
        ws.chrome.status.as_ref().map(|s| (s.level, s.text.clone()))
    }

    fn asking(ws: &Workspace) -> bool {
        matches!(ws.modals.last(), Some(crate::modal::Modal::Confirm(_)))
    }

    #[test]
    fn 一覧で小文字の_d_を打っても何も起きない() {
        let (mut ws, mut svc) = opened();
        let dir = tempfile::tempdir().unwrap();
        let id = spawn(&mut ws, dir.path());
        list(&mut ws, dir.path(), &[&id]);
        type_text(&mut ws, &mut svc, "d");
        assert!(ws.modals.is_empty());
        assert!(present(&ws, &id));
    }

    #[test]
    fn 一覧に居ると気づかずに打った文では止まらない() {
        for text in ["ready", "already", "Deploy it", "Do you"] {
            let (mut ws, mut svc) = opened();
            let dir = tempfile::tempdir().unwrap();
            let id = spawn(&mut ws, dir.path());
            list(&mut ws, dir.path(), &[&id]);
            type_text(&mut ws, &mut svc, text);
            assert!(ws.modals.is_empty(), "{text}");
            assert!(present(&ws, &id), "{text}");
        }
    }

    #[test]
    fn 大文字の_d_に_y_が続く語を含む文は止める確認に答えてしまう() {
        for text in ["ready Dynamic", "READY"] {
            let (mut ws, mut svc) = opened();
            let dir = tempfile::tempdir().unwrap();
            let id = spawn(&mut ws, dir.path());
            list(&mut ws, dir.path(), &[&id]);
            type_text(&mut ws, &mut svc, text);
            assert!(!present(&ws, &id), "{text}");
        }
    }

    #[test]
    fn 文中の_j_k_で選択が動いた後に_d_y_が来ると動いた先の行が止まる() {
        let (mut ws, mut svc) = opened();
        let dir = tempfile::tempdir().unwrap();
        let a = spawn(&mut ws, dir.path());
        let b = spawn(&mut ws, dir.path());
        list(&mut ws, dir.path(), &[&a, &b]);
        let order: Vec<RowKey> = ws
            .panels
            .fleet
            .visual_rows()
            .into_iter()
            .map(|r| r.key)
            .collect();
        let [RowKey::Panel(first), RowKey::Panel(last)] = order.as_slice() else {
            panic!("{order:?}");
        };
        type_text(&mut ws, &mut svc, "G");
        assert_eq!(
            ws.panels.fleet.selected_row().unwrap().key,
            RowKey::Panel(last.clone())
        );
        type_text(&mut ws, &mut svc, "I think Dynamo");
        assert!(!present(&ws, first));
        assert!(present(&ws, last));
    }

    #[test]
    fn 止める確認は_y_以外のキーでやめる() {
        for key in [
            KeyCode::Right,
            KeyCode::Left,
            KeyCode::Char('l'),
            KeyCode::Char('h'),
            KeyCode::Tab,
            KeyCode::Enter,
            KeyCode::Char('x'),
            KeyCode::Char('n'),
            KeyCode::Esc,
        ] {
            let (mut ws, mut svc) = opened();
            let dir = tempfile::tempdir().unwrap();
            let id = spawn(&mut ws, dir.path());
            list(&mut ws, dir.path(), &[&id]);
            type_text(&mut ws, &mut svc, "D");
            assert!(asking(&ws), "{key:?}");
            on_key(&mut ws, &mut svc, KeyEvent::new(key, KeyModifiers::NONE));
            assert!(ws.modals.is_empty(), "{key:?}");
            assert!(present(&ws, &id), "{key:?}");
            assert_eq!(ws.focus, Focus::Fleet, "{key:?}");
        }
    }

    #[test]
    fn 確認で_y_を押すとこの窓のセッションを止め_一覧からも_claude_区画からもすぐ外す() {
        let (mut ws, mut svc) = opened();
        let dir = tempfile::tempdir().unwrap();
        let other = spawn(&mut ws, dir.path());
        let shown = spawn(&mut ws, dir.path());
        list(&mut ws, dir.path(), &[&other, &shown]);
        while ws.panels.fleet.selected_row().unwrap().key != RowKey::Panel(shown.clone()) {
            type_text(&mut ws, &mut svc, "j");
        }
        type_text(&mut ws, &mut svc, "Dy");
        assert!(!present(&ws, &shown));
        assert!(present(&ws, &other));
        assert!(
            ws.panels
                .fleet
                .visual_rows()
                .iter()
                .all(|r| r.key != RowKey::Panel(shown.clone()))
        );
        assert_ne!(ws.panels.terminal.visible_claude(), Some(shown.as_str()));
        assert_eq!(
            status_of(&ws),
            Some((StatusLevel::Success, "Stopped the session.".into()))
        );
    }

    #[test]
    fn 別の_worktree_のセッションも止められる() {
        let (mut ws, mut svc) = opened();
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let away = spawn(&mut ws, first.path());
        let here = spawn(&mut ws, second.path());
        apply(
            &mut ws,
            &mut svc,
            vec![Effect::StopFleetSession(StopTarget::Panel(away.clone()))],
        );
        assert!(!present(&ws, &away));
        assert!(present(&ws, &here));
        assert_eq!(ws.panels.terminal.visible_claude(), Some(here.as_str()));
    }

    #[test]
    fn もう無いセッションを止めるともう終わっていたと出す() {
        let (mut ws, mut svc) = opened();
        apply(
            &mut ws,
            &mut svc,
            vec![Effect::StopFleetSession(StopTarget::Panel("gone".into()))],
        );
        assert_eq!(
            status_of(&ws),
            Some((StatusLevel::Info, "That session had already ended.".into()))
        );
    }

    #[test]
    fn 終わったのにタブが残っているセッションはタブを閉じてもう終わっていたと出す() {
        let (mut ws, mut svc) = opened();
        let dir = tempfile::tempdir().unwrap();
        let id = spawn(&mut ws, dir.path());
        list(&mut ws, dir.path(), &[&id]);
        apply(&mut ws, &mut svc, vec![Effect::Focus(Focus::FleetSession)]);
        on_paste(&mut ws, &mut svc, "exit\n".into());
        crate::testing::wait_for(|| ws.panels.terminal.session_count() == 0);
        assert!(present(&ws, &id));
        apply(
            &mut ws,
            &mut svc,
            vec![Effect::StopFleetSession(StopTarget::Panel(id.clone()))],
        );
        assert!(!present(&ws, &id));
        assert_eq!(
            status_of(&ws),
            Some((StatusLevel::Info, "That session had already ended.".into()))
        );
    }

    #[test]
    fn 右の区画では_d_も左キーも_claude_に届く() {
        let (mut ws, mut svc) = opened();
        let dir = tempfile::tempdir().unwrap();
        let id = spawn(&mut ws, dir.path());
        list(&mut ws, dir.path(), &[&id]);
        apply(&mut ws, &mut svc, vec![Effect::Focus(Focus::FleetSession)]);
        for key in [
            KeyEvent::new(KeyCode::Char('D'), KeyModifiers::SHIFT),
            KeyEvent::new(KeyCode::Left, KeyModifiers::NONE),
        ] {
            assert_eq!(route(&mut ws, key), Routed::ForwardToPty(key));
        }
    }

    #[test]
    fn 外の行で_tab_を押した後に打った文でも止める確認は残らない() {
        let (mut ws, mut svc) = opened();
        let outside = live(9, "/r/a", "s9", Status::Working, Owner::Elsewhere);
        ws.apply_fleet(status(vec![outside]), Vec::new());
        on_key(
            &mut ws,
            &mut svc,
            KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE),
        );
        assert_eq!(ws.focus, Focus::Fleet);
        type_text(&mut ws, &mut svc, "Deploy");
        assert!(ws.modals.is_empty());
    }

    #[test]
    fn 打っていたセッションが消えて一覧に戻った後の文でも止まらない() {
        let (mut ws, mut svc) = opened();
        let dir = tempfile::tempdir().unwrap();
        let a = spawn(&mut ws, dir.path());
        let b = spawn(&mut ws, dir.path());
        list(&mut ws, dir.path(), &[&a, &b]);
        apply(&mut ws, &mut svc, vec![Effect::Focus(Focus::FleetSession)]);
        let typing = ws.fleet_session().unwrap();
        let rest = if typing == a { b } else { a };
        list(&mut ws, dir.path(), &[&rest]);
        assert_eq!(ws.focus, Focus::Fleet);
        type_text(&mut ws, &mut svc, "Do you");
        assert!(ws.modals.is_empty());
        assert!(present(&ws, &rest));
    }

    #[test]
    fn 止めた直後の読み直しが止める前の写しを持ち帰ると_pid_の行で一度出る() {
        let (mut ws, mut svc) = opened();
        let dir = tempfile::tempdir().unwrap();
        let id = spawn(&mut ws, dir.path());
        let cwd = dir.path().to_string_lossy().to_string();
        let before = || live(7, &cwd, "s1", Status::Working, Owner::Conductor);
        ws.apply_fleet(status(vec![before()]), vec![own(&id, &cwd, Some("s1"))]);
        apply(
            &mut ws,
            &mut svc,
            vec![Effect::StopFleetSession(StopTarget::Panel(id.clone()))],
        );
        assert!(ws.panels.fleet.visual_rows().is_empty());
        ws.apply_fleet(status(vec![before()]), Vec::new());
        let rows = ws.panels.fleet.visual_rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].key, RowKey::Pid(7));
        assert_eq!(rows[0].place, Place::OtherConductor);
    }
}
