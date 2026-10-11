//! The config window through the model: opening and leaving it, the key
//! flows, the save into a temp `RIVAL_HOME`, the debounced probe, the check
//! rows, and the golden frames at 80×24 and 140×40. The proxy and the check
//! are fakes (`testkit::fake_models`, `testkit::fake_check`); nothing leaves
//! the process.

use super::*;
use crate::check::CheckRow;
use crate::tui::config_form::{Field, ProbeState, Section};
use crate::tui::testkit::{
    PROXY_YAML, TEST_KEY, assert_golden, config_model, config_seed, draw, drive, fixed_now,
    frame_text, golden_frame, golden_path, harness, key, list_model, rows, type_text,
};

fn form(m: &Model) -> &ConfigForm {
    m.config.as_ref().expect("the config window is open")
}

/// The commands of one update.
fn cmds(m: &mut Model, msg: Msg) -> Vec<Cmd> {
    m.update(msg)
}

#[test]
fn c_opens_the_window_from_the_list_and_esc_goes_back() {
    let h = harness();
    let mut m = list_model(Vec::new(), 100, 30);
    m.set_config_seed(config_seed(&h, Some(PROXY_YAML), Some(TEST_KEY)));
    let out = cmds(&mut m, key("c"));
    assert_eq!(m.mode, Mode::Config);
    assert!(
        out.iter().any(|c| matches!(c, Cmd::Job(Job::Probe(_)))),
        "the window asks the proxy at once: {out:?}"
    );
    assert!(frame_text(&m).contains("rival · config"));
    drive(&mut m, &h.env, [key("esc")]);
    assert_eq!(m.mode, Mode::List);
    assert!(m.config.is_none() && !m.quitting());
}

#[test]
fn rival_config_quits_on_esc_and_never_watches() {
    let h = harness();
    let mut m = config_model(&h, Some(PROXY_YAML), Some(TEST_KEY), 80, 24);
    assert!(m.config_only && m.loaded);
    assert_eq!(form(&m).probe.state, ProbeState::Online(6));
    assert_eq!(cmds(&mut m, key("esc")), [Cmd::Quit]);
    assert!(m.quitting());
}

#[test]
fn an_unsaved_draft_asks_before_leaving() {
    let h = harness();
    let mut m = config_model(&h, Some(PROXY_YAML), Some(TEST_KEY), 80, 24);
    drive(&mut m, &h.env, [key("space"), key("esc")]);
    assert!(form(&m).confirm_exit);
    assert!(frame_text(&m).contains("save changes?"));
    drive(&mut m, &h.env, [key("esc")]);
    assert!(!m.quitting() && !form(&m).confirm_exit);
    // n discards: the file stays as it was.
    drive(&mut m, &h.env, [key("esc"), key("n")]);
    assert!(m.quitting());
    let text = std::fs::read_to_string(h.paths().config_file()).unwrap();
    assert_eq!(text, PROXY_YAML);
}

#[test]
fn y_in_the_confirm_bar_saves_then_quits() {
    let h = harness();
    let mut m = config_model(&h, Some(PROXY_YAML), Some(TEST_KEY), 80, 24);
    drive(
        &mut m,
        &h.env,
        [key("down"), key("space"), key("q"), key("y")],
    );
    assert!(m.quitting(), "the save landed and the window closed");
    let text = std::fs::read_to_string(h.paths().config_file()).unwrap();
    assert!(
        text.starts_with(rival_core::config::write::HEADER),
        "{text}"
    );
    assert!(text.contains("codex:\n    enabled: false"), "{text}");
    assert!(
        text.contains("reviewer: Review like a staff engineer."),
        "{text}"
    );
    // The first rewrite of a hand-written file keeps a backup.
    let bak = h.paths().root.join("config.yaml.bak");
    assert_eq!(std::fs::read_to_string(bak).unwrap(), PROXY_YAML);
}

#[test]
fn s_writes_the_draft_and_the_typed_key() {
    let h = harness();
    let mut m = config_model(&h, Some(PROXY_YAML), None, 140, 40);
    // Key: enter, paste, enter. Then a prefix by free text.
    drive(&mut m, &h.env, [key("down"), key("down"), key("down")]);
    assert_eq!(form(&m).field(), Some(Field::Key));
    drive(&mut m, &h.env, [key("enter")]);
    assert_eq!(m.mode, Mode::ConfigEdit);
    drive(
        &mut m,
        &h.env,
        [Msg::Paste("sk-new-key-5566".into()), key("enter")],
    );
    assert_eq!(m.mode, Mode::Config);
    let frame = frame_text(&m);
    assert!(!frame.contains("sk-new-key"), "the key never shows");
    assert!(frame.contains("••••••••5566"), "{frame}");
    drive(&mut m, &h.env, [key("down"), key("e")]);
    drive(&mut m, &h.env, type_text("x"));
    drive(&mut m, &h.env, [key("enter"), key("s")]);
    let f = form(&m);
    assert!(!f.dirty() && !f.saving, "{f:?}");
    assert_eq!(
        f.notice.as_ref().unwrap().text,
        "saved ~/.rival/config.yaml"
    );
    let text = std::fs::read_to_string(h.paths().config_file()).unwrap();
    assert!(text.contains("model_prefix: emcd2_x"), "{text}");
    let key_path = h.paths().proxy_key_file();
    assert_eq!(
        std::fs::read_to_string(&key_path).unwrap(),
        "sk-new-key-5566\n"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&key_path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    assert!(frame_text(&m).contains("0600 ✓"));
}

#[test]
fn save_is_blocked_while_a_field_is_invalid() {
    let h = harness();
    let mut m = config_model(&h, Some(PROXY_YAML), Some(TEST_KEY), 80, 24);
    drive(
        &mut m,
        &h.env,
        [key("down"), key("down"), key("enter"), key("end")],
    );
    drive(&mut m, &h.env, type_text(" bad"));
    drive(&mut m, &h.env, [key("enter")]);
    assert!(
        frame_text(&m).contains("✗ the URL has a space"),
        "{}",
        frame_text(&m)
    );
    let out = cmds(&mut m, key("s"));
    assert!(out.iter().all(|c| !matches!(c, Cmd::Job(Job::Save(_)))));
    assert!(frame_text(&m).contains("save blocked: URL — the URL has a space"));
    // The key bar says so before s is pressed, too.
    drive(&mut m, &h.env, [key("j")]);
    assert!(frame_text(&m).contains("s save blocked"));
    let text = std::fs::read_to_string(h.paths().config_file()).unwrap();
    assert_eq!(text, PROXY_YAML);
}

#[test]
fn typing_a_url_probes_once_after_the_pause() {
    let h = harness();
    let mut m = config_model(&h, Some(PROXY_YAML), Some(TEST_KEY), 80, 24);
    drive(&mut m, &h.env, [key("down"), key("down"), key("enter")]);
    let mut timers = Vec::new();
    for msg in type_text("0").into_iter().chain(type_text("1")) {
        for cmd in m.update(msg) {
            match cmd {
                Cmd::After(delay, msg) => timers.push((delay, msg)),
                Cmd::Job(job) => panic!("no call while typing: {job:?}"),
                _ => {}
            }
        }
    }
    assert_eq!(timers.len(), 2);
    assert!(timers.iter().all(|(d, _)| *d == Duration::from_millis(600)));
    // The first pause is stale: only the last one asks.
    let (_, first) = timers.remove(0);
    assert!(m.update(first).is_empty());
    let (_, last) = timers.remove(0);
    let out = m.update(last);
    let Some(Cmd::Job(Job::Probe(req))) = out.first() else {
        panic!("{out:?}");
    };
    assert_eq!(req.cfg.proxy_url().unwrap(), "http://127.0.0.1:831701");
}

#[test]
fn a_check_streams_rows_and_records_the_time() {
    let h = harness();
    let mut m = config_model(&h, Some(PROXY_YAML), Some(TEST_KEY), 140, 40);
    let out = cmds(&mut m, key("c"));
    assert!(out.iter().any(|c| matches!(c, Cmd::Job(Job::Check(_)))));
    assert!(form(&m).check.running);
    // The fake check through the job runner: rows, then the report.
    let Some(Cmd::Job(job)) = out.into_iter().find(|c| matches!(c, Cmd::Job(_))) else {
        unreachable!()
    };
    let rows_seen = std::sync::Mutex::new(Vec::new());
    let done = h
        .env
        .run_with(job, &|msg| rows_seen.lock().unwrap().push(msg))
        .into_msg()
        .unwrap();
    let rows_seen = rows_seen.into_inner().unwrap();
    assert_eq!(rows_seen.len(), 5);
    for msg in rows_seen {
        m.update(msg);
    }
    assert!(form(&m).check.running, "rows alone do not end the run");
    m.update(done);
    let f = form(&m);
    assert!(!f.check.running);
    assert_eq!(f.check.passed(), 5);
    assert!(frame_text(&m).contains("last run 12:00 · 5 of 5 ok"));
}

#[test]
fn quitting_cancels_a_running_check() {
    let h = harness();
    let mut m = config_model(&h, Some(PROXY_YAML), Some(TEST_KEY), 80, 24);
    let out = cmds(&mut m, key("a"));
    let Some(Cmd::Job(Job::Check(req))) = out.iter().find(|c| matches!(c, Cmd::Job(Job::Check(_))))
    else {
        panic!("{out:?}");
    };
    assert_eq!(req.targets.len(), 7, "a checks every model");
    assert!(!req.ctx.is_done());
    m.update(key("ctrl+c"));
    assert!(req.ctx.is_done());
}

#[test]
fn the_spinner_runs_while_a_check_does() {
    let h = harness();
    let mut m = config_model(&h, Some(PROXY_YAML), Some(TEST_KEY), 80, 24);
    m.spinning = false;
    let out = cmds(&mut m, key("c"));
    assert!(out.contains(&Cmd::Spin));
    assert_eq!(m.update(Msg::SpinTick), [Cmd::Spin]);
}

// --- golden frames ------------------------------------------------------------

/// Five rows in every state: a pass, an unexpected reply, a running row, a
/// failure with a hint and an account at its limit.
fn mixed_rows(m: &mut Model) {
    m.update(key("c"));
    let run = form(m).check.run;
    let row = |name: &str, route: &str, wire: &str| CheckRow {
        name: name.into(),
        route: route.into(),
        wire_model: wire.into(),
        called: true,
        ..CheckRow::default()
    };
    let rows = [
        CheckRow {
            ok: true,
            ms: 2140,
            reply: "ok".into(),
            ..row("claude", "proxy", "emcd2_/claude-opus-5-5")
        },
        CheckRow {
            ok: true,
            ms: 1830,
            reply: "Sure! ok.".into(),
            unexpected: true,
            ..row("fable", "proxy", "emcd2_/claude-fable-5-1")
        },
        CheckRow {
            called: false,
            error: "proxy does not serve gpt-6.1-sol; it serves no gpt-6.1-sol — log in Codex on the proxy (-codex-device-login)".into(),
            hint: "set proxy.codex.model_prefix".into(),
            ..row("sol", "proxy", "gpt-6.1-sol")
        },
        CheckRow {
            ms: 4020,
            limit: true,
            error: "429 monthly spend limit reached".into(),
            hint: "the account is at its limit; try again after the reset".into(),
            ..row("k3", "direct", "moonshotai/kimi-k3")
        },
    ];
    for r in rows {
        m.update(Msg::CheckRow {
            run,
            row: Box::new(r),
        });
    }
}

/// The config frames under `src/tui/testdata`: fixed seeds in a temp home
/// shown as `~`, the fake proxy and a pinned clock.
fn golden_cases() -> Vec<(String, String)> {
    let h = harness();
    let mut cases = Vec::new();
    let mut frame = |name: &str, m: &Model| {
        let (w, ht) = (m.lay.width as u16, m.lay.height as u16);
        cases.push((
            format!("config_{name}_{w}x{ht}"),
            golden_frame(&draw(m, w, ht)),
        ));
    };
    for (w, ht) in [(80, 24), (140, 40)] {
        let base = || config_model(&h, Some(PROXY_YAML), Some(TEST_KEY), w, ht);
        frame("proxy", &base());

        let mut models = base();
        models.update(key("tab"));
        models.update(key("down"));
        models.update(key("down"));
        frame("models", &models);

        let mut review = base();
        review.update(key("3"));
        frame("review", &review);

        let mut check = base();
        mixed_rows(&mut check);
        check.update(key("4"));
        frame("check", &check);

        let mut dirty = base();
        dirty.update(key("down"));
        dirty.update(key("space"));
        dirty.update(key("down"));
        dirty.update(key("down"));
        dirty.update(key("down"));
        dirty.update(key("right"));
        mixed_rows(&mut dirty);
        frame("dirty", &dirty);

        let mut invalid = base();
        drive(
            &mut invalid,
            &h.env,
            [key("down"), key("down"), key("enter"), key("end")],
        );
        drive(&mut invalid, &h.env, type_text("/x y"));
        drive(&mut invalid, &h.env, [key("enter")]);
        frame("invalid", &invalid);
    }
    let fresh = harness();
    let mut key_edit = config_model(&fresh, None, None, 80, 24);
    for k in ["down", "down", "down", "enter"] {
        key_edit.update(key(k));
    }
    key_edit.update(Msg::Paste("abcdefgh".into()));
    frame("key_edit", &key_edit);

    let mut confirm = config_model(&h, Some(PROXY_YAML), Some(TEST_KEY), 80, 24);
    confirm.update(key("space"));
    confirm.update(key("esc"));
    frame("confirm", &confirm);

    // A saved prefix the proxy does not serve: a yellow warning, not an
    // error.
    let unserved_yaml = PROXY_YAML.replace("emcd2_", "team9");
    let mut unserved = config_model(&h, Some(&unserved_yaml), Some(TEST_KEY), 80, 24);
    unserved.update(key("j"));
    frame("unserved", &unserved);

    let mut help = config_model(&h, Some(PROXY_YAML), Some(TEST_KEY), 140, 40);
    help.update(key("?"));
    frame("help", &help);
    cases
}

#[test]
fn config_frames_match_golden() {
    for (name, frame) in golden_cases() {
        assert_golden(&name, &frame);
    }
}

#[test]
#[ignore = "rewrites the committed golden frames"]
fn regenerate_config_golden_frames() {
    for (name, frame) in golden_cases() {
        let path = golden_path(&name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, frame).unwrap();
    }
}

/// Nothing draws over the box at 80×24: every row keeps its border, the
/// corners stand, and no line runs past the right edge.
#[test]
fn nothing_overflows_at_80x24() {
    let h = harness();
    for section in ["1", "2", "3", "4"] {
        let mut m = config_model(&h, Some(PROXY_YAML), Some(TEST_KEY), 80, 24);
        mixed_rows(&mut m);
        m.update(key(section));
        let r = rows(&draw(&m, 80, 24));
        assert_eq!(r.len(), 24);
        assert!(
            r[0].starts_with('╭') && r[0].ends_with('╮'),
            "{section}: {}",
            r[0]
        );
        assert!(
            r[23].starts_with('╰') && r[23].ends_with('╯'),
            "{section}: {}",
            r[23]
        );
        for (y, row) in r.iter().enumerate().skip(1).take(22) {
            let last = row.chars().last().unwrap();
            assert!(
                matches!(last, '│' | '┤'),
                "{section}: row {y} lost its border: {row:?}"
            );
            assert_eq!(crate::tui::text::width(row), 80, "row {y}");
        }
    }
}

#[test]
fn narrow_frames_fold_the_sections_into_tabs() {
    let h = harness();
    let narrow = config_model(&h, Some(PROXY_YAML), Some(TEST_KEY), 99, 30);
    let r = rows(&draw(&narrow, 99, 30));
    assert!(
        r[1].contains("1 Proxy  2 Models  3 Review  4 Check"),
        "{}",
        r[1]
    );
    let wide = config_model(&h, Some(PROXY_YAML), Some(TEST_KEY), 100, 30);
    let r = rows(&draw(&wide, 100, 30));
    assert!(r[2].contains("▸ Proxy"), "{}", r[2]);
    assert!(!r[1].contains("2 Models"));
}

#[test]
fn the_title_shows_the_path_and_unsaved() {
    let h = harness();
    let mut m = config_model(&h, Some(PROXY_YAML), Some(TEST_KEY), 140, 40);
    let top = rows(&draw(&m, 140, 40))[0].clone();
    assert!(top.contains("rival · config") && top.contains("~/.rival/config.yaml"));
    assert!(!top.contains("unsaved"));
    m.update(key("space"));
    let top = rows(&draw(&m, 140, 40))[0].clone();
    assert!(top.contains("~/.rival/config.yaml  ● unsaved ─╮"), "{top}");
    let _ = fixed_now();
    let _ = Section::Proxy;
}

#[test]
fn a_late_answer_never_reaches_the_next_window() {
    let h = harness();
    let mut m = list_model(Vec::new(), 100, 30);
    m.set_config_seed(config_seed(&h, Some(PROXY_YAML), Some(TEST_KEY)));
    let out = cmds(&mut m, key("c"));
    let probe = out.iter().find_map(|c| match c {
        Cmd::Job(Job::Probe(req)) => Some(req.seq),
        _ => None,
    });
    m.update(key("c"));
    let run = form(&m).check.run;
    m.update(key("esc"));
    m.update(key("c"));
    // The old window's probe and check answers arrive now.
    m.update(Msg::ProxyModels(crate::tui::config_check::ProbeResult {
        seq: probe.unwrap(),
        result: Err("old".into()),
    }));
    m.update(Msg::CheckRow {
        run,
        row: Box::new(CheckRow {
            name: "codex".into(),
            ..CheckRow::default()
        }),
    });
    let f = form(&m);
    assert!(f.check.slots.is_empty(), "{:?}", f.check.slots);
    assert!(!matches!(f.probe.state, ProbeState::Offline(_)));
}
