//! The config window's pure state: keys, validation, the patch and the
//! draft config. Every seed lives in a temp home; nothing is written.

use super::*;
use crate::check::{CODEX, ProxyLine};
use crate::tui::testkit::{
    FAKE_PROXY_IDS, PROXY_YAML, TEST_KEY, config_seed, fixed_now, harness, press,
};

fn form(yaml: Option<&str>) -> (crate::tui::testkit::Harness, ConfigForm) {
    let h = harness();
    let seed = config_seed(&h, yaml, Some(TEST_KEY));
    (h, ConfigForm::new(seed))
}

/// Applies each key by name.
fn keys(f: &mut ConfigForm, names: &[&str]) -> Vec<Option<Effect>> {
    names.iter().map(|k| f.apply(k, &press(k))).collect()
}

fn typed(f: &mut ConfigForm, text: &str) -> Vec<Option<Effect>> {
    text.chars()
        .map(|c| {
            let k = c.to_string();
            f.apply(&k, &press(&k))
        })
        .collect()
}

fn focus(f: &mut ConfigForm, field: Field) {
    f.section = Section::ALL
        .into_iter()
        .find(|s| s.fields().contains(&field))
        .unwrap();
    f.focus = f.section.fields().iter().position(|x| *x == field).unwrap();
}

#[test]
fn a_fresh_form_is_the_saved_config() {
    let (_h, f) = form(Some(PROXY_YAML));
    assert!(!f.dirty());
    assert_eq!(f.patch(), json!({}));
    assert!(f.errors.is_empty() && f.general_error.is_none(), "{f:?}");
    assert_eq!(f.field(), Some(Field::ClaudeRoute));
    assert_eq!(f.seed.path_shown, "~/.rival/config.yaml");
    assert!(matches!(&f.seed.key, KeyStatus::Set { tail, env: false, .. } if tail == "a91f"));
}

#[test]
fn tab_and_arrows_move_between_sections_and_fields() {
    let (_h, mut f) = form(None);
    keys(&mut f, &["down", "down", "j"]);
    assert_eq!(f.field(), Some(Field::Key));
    keys(&mut f, &["down", "down", "down", "down"]);
    assert_eq!(
        f.field(),
        Some(Field::CodexPrefix),
        "the focus stops at the end"
    );
    keys(&mut f, &["tab"]);
    assert_eq!(
        (f.section, f.field()),
        (Section::Models, Some(Field::Model(0)))
    );
    keys(&mut f, &["shift+tab", "shift+tab"]);
    assert_eq!(f.section, Section::Check);
    assert_eq!(f.field(), None);
    keys(&mut f, &["3"]);
    assert_eq!(f.field(), Some(Field::AutoFix));
}

#[test]
fn a_toggle_changes_the_draft_and_the_patch() {
    let (_h, mut f) = form(Some(PROXY_YAML));
    keys(&mut f, &["space"]);
    assert!(!f.draft.proxy.claude.enabled);
    assert!(f.dirty());
    assert_eq!(f.patch(), json!({"proxy.claude.enabled": false}));
    // Back to the saved value: nothing to write.
    keys(&mut f, &["enter"]);
    assert!(!f.dirty());
}

#[test]
fn an_enabled_route_needs_a_url() {
    let (_h, mut f) = form(None);
    keys(&mut f, &["space"]);
    assert!(
        f.error(Field::Url).unwrap().contains("set its URL"),
        "{f:?}"
    );
    assert_eq!(f.apply("s", &press("s")), None, "save is blocked");
    let notice = f.notice.clone().unwrap();
    assert_eq!(notice.kind, NoticeKind::Error);
    assert!(
        notice.text.starts_with("save blocked: URL — "),
        "{notice:?}"
    );
}

#[test]
fn a_url_edit_commits_on_enter_and_shows_its_error() {
    let (_h, mut f) = form(Some(PROXY_YAML));
    focus(&mut f, Field::Url);
    keys(&mut f, &["enter"]);
    assert!(f.editing());
    // ctrl+u clears; each change asks for a debounced probe.
    f.apply(
        "ctrl+u",
        &KeyEvent::new(
            crossterm::event::KeyCode::Char('u'),
            crossterm::event::KeyModifiers::CONTROL,
        ),
    );
    let effects = typed(&mut f, "ftp://x");
    assert!(
        effects
            .iter()
            .all(|e| *e == Some(Effect::Probe { debounce: true }))
    );
    assert_eq!(
        f.draft.proxy.url, "http://127.0.0.1:8317",
        "the draft waits for enter"
    );
    assert_eq!(
        keys(&mut f, &["enter"]),
        [Some(Effect::Probe { debounce: false })]
    );
    assert!(!f.editing());
    assert_eq!(f.draft.proxy.url, "ftp://x");
    assert_eq!(f.error(Field::Url), Some("use an http:// or https:// URL"));
    assert!(f.blocked().is_some());
    // A good URL clears it.
    keys(&mut f, &["enter"]);
    f.paste("ignored? no: a paste lands in the edit");
    keys(&mut f, &["esc"]);
    assert_eq!(f.draft.proxy.url, "ftp://x", "esc drops the edit");
    keys(&mut f, &["enter"]);
    f.edit.as_mut().unwrap().input.set_value("");
    assert_eq!(
        f.paste("https://proxy.example/v1/"),
        Some(Effect::Probe { debounce: true })
    );
    keys(&mut f, &["enter"]);
    assert_eq!(f.error(Field::Url), None);
    assert_eq!(f.patch(), json!({"proxy.url": "https://proxy.example/v1/"}));
}

#[test]
fn d_imports_the_base_url_from_the_environment() {
    let (_h, mut f) = form(None);
    focus(&mut f, Field::Url);
    keys(&mut f, &["d"]);
    assert_eq!(
        f.notice.as_ref().unwrap().text,
        "ANTHROPIC_BASE_URL is not set"
    );
    f.seed.base_url_env = "http://localhost:8317".into();
    assert_eq!(
        keys(&mut f, &["d"]),
        [Some(Effect::Probe { debounce: false })]
    );
    assert_eq!(f.draft.proxy.url, "http://localhost:8317");
}

#[test]
fn the_prefix_picker_lists_the_prefixes_that_serve_the_provider() {
    let (_h, mut f) = form(Some(PROXY_YAML));
    // Before the proxy answers: the draft's value and none.
    assert_eq!(f.prefix_choices(ProxyProvider::Claude), ["emcd2_", ""]);
    let seq = f.bump_probe();
    f.probe_done(
        seq,
        Ok(FAKE_PROXY_IDS.iter().map(|s| s.to_string()).collect()),
    );
    assert_eq!(f.probe.state, ProbeState::Online(6));
    // The proxy serves no bare Claude id: no `none` for Claude.
    assert_eq!(f.prefix_choices(ProxyProvider::Claude), ["emcd2_", "emcd_"]);
    assert_eq!(
        f.prefix_choices(ProxyProvider::Codex),
        [""],
        "codex ids have no prefix"
    );
    assert_eq!(f.prefix_warning(ProxyProvider::Claude), None);
    focus(&mut f, Field::ClaudePrefix);
    keys(&mut f, &["right"]);
    assert_eq!(f.draft.proxy.claude.model_prefix, "emcd_");
    keys(&mut f, &["right"]);
    assert_eq!(
        f.draft.proxy.claude.model_prefix, "emcd2_",
        "wraps, never none"
    );
    keys(&mut f, &["left"]);
    assert_eq!(f.draft.proxy.claude.model_prefix, "emcd_");
    assert_eq!(f.patch(), json!({"proxy.claude.model_prefix": "emcd_"}));
    assert_eq!(f.wire(&MODELS[3]), "emcd_/claude-fable-5-1");
    // e types a prefix the proxy does not list; the trailing / goes. It is
    // kept, with a warning that does not block the save.
    keys(&mut f, &["e"]);
    f.edit.as_mut().unwrap().input.set_value("");
    typed(&mut f, "team9/");
    keys(&mut f, &["enter"]);
    assert_eq!(f.draft.proxy.claude.model_prefix, "team9");
    assert_eq!(
        f.prefix_choices(ProxyProvider::Claude),
        ["emcd2_", "emcd_", "team9"]
    );
    assert_eq!(
        f.prefix_warning(ProxyProvider::Claude).as_deref(),
        Some("the proxy does not serve claude-opus-5-5 as team9/claude-opus-5-5")
    );
    assert_eq!(f.blocked(), None);
    assert_eq!(keys(&mut f, &["s"]), [Some(Effect::Save)]);
    f.saving = false;
    // A space is an error.
    keys(&mut f, &["e"]);
    typed(&mut f, " x");
    keys(&mut f, &["enter"]);
    assert_eq!(
        f.error(Field::ClaudePrefix),
        Some("a prefix has no spaces and no /")
    );
}

#[test]
fn none_is_offered_only_when_the_proxy_serves_the_bare_ids() {
    let (_h, mut f) = form(Some("proxy:\n  url: http://127.0.0.1:8317\n"));
    // Offline: every value is open.
    assert_eq!(f.prefix_choices(ProxyProvider::Claude), [""]);
    assert_eq!(f.prefix_warning(ProxyProvider::Claude), None);
    let seq = f.bump_probe();
    let ids = [
        "zz_/claude-fable-5-1",
        "aa_/claude-opus-5-5",
        "x_/gpt-6-astra",
        "gpt-6.1-sol",
        "other/llama",
    ];
    f.probe_done(seq, Ok(ids.iter().map(|s| s.to_string()).collect()));
    // The saved "" stays choosable, with a warning: no bare Claude id.
    assert_eq!(f.prefix_choices(ProxyProvider::Claude), ["aa_", "zz_", ""]);
    assert_eq!(
        f.prefix_warning(ProxyProvider::Claude).as_deref(),
        Some("the proxy does not serve claude-opus-5-5 as claude-opus-5-5")
    );
    // Codex: x_ serves astra, and sol is served bare, so none is offered.
    assert_eq!(f.prefix_choices(ProxyProvider::Codex), ["x_", ""]);
    assert_eq!(
        f.prefix_warning(ProxyProvider::Codex).as_deref(),
        Some("the proxy does not serve gpt-6-astra as gpt-6-astra")
    );
    // Once on a served prefix, "" is gone from the Claude picker.
    f.draft.proxy.claude.model_prefix = "aa_".into();
    assert_eq!(f.prefix_choices(ProxyProvider::Claude), ["aa_", "zz_"]);
    assert_eq!(
        f.prefix_warning(ProxyProvider::Claude).as_deref(),
        Some("the proxy does not serve claude-fable-5-1 as aa_/claude-fable-5-1")
    );
}

#[test]
fn a_stale_probe_answer_is_dropped() {
    let (_h, mut f) = form(Some(PROXY_YAML));
    let old = f.bump_probe();
    let new = f.bump_probe();
    f.probe_done(old, Ok(vec!["x/claude-opus-5-5".into()]));
    assert_eq!(f.probe.state, ProbeState::Idle);
    f.probe_done(
        new,
        Err("proxy unreachable at http://127.0.0.1:8317: refused".into()),
    );
    assert!(matches!(f.probe.state, ProbeState::Offline(_)));
}

#[test]
fn the_probe_uses_the_url_being_typed_and_needs_one() {
    let (_h, mut f) = form(None);
    f.bump_probe();
    assert_eq!(f.probe_request(), None);
    assert_eq!(f.probe.state, ProbeState::Idle);
    focus(&mut f, Field::Url);
    keys(&mut f, &["enter"]);
    typed(&mut f, "http://127.0.0.1:9");
    let req = f.probe_request().unwrap();
    assert_eq!(req.cfg.proxy_url().unwrap(), "http://127.0.0.1:9");
    assert_eq!(req.cfg.proxy_key().unwrap().unwrap().secret(), TEST_KEY);
    assert_eq!(f.probe.state, ProbeState::Waiting);
}

#[test]
fn effort_pickers_step_through_each_ladder() {
    let (_h, mut f) = form(None);
    keys(&mut f, &["tab"]);
    assert_eq!(f.effort(&MODELS[0]), "xhigh");
    keys(&mut f, &["right"]);
    assert_eq!(f.effort(&MODELS[0]), "ultra");
    keys(&mut f, &["right"]);
    assert_eq!(f.effort(&MODELS[0]), "low", "the ladder wraps");
    assert_eq!(f.patch(), json!({"efforts.codex": "low"}));
    keys(&mut f, &["left", "left"]);
    assert_eq!(f.patch(), json!({}), "the default again is no change");
    // Grok steps low, medium, high.
    keys(&mut f, &["down", "down", "down", "down", "right"]);
    assert_eq!(f.field(), Some(Field::Model(4)));
    assert_eq!(f.effort(&MODELS[4]), "low");
    keys(&mut f, &["left", "left"]);
    assert_eq!(f.effort(&MODELS[4]), "medium");
    assert_eq!(f.patch(), json!({"efforts.grok": "medium"}));
}

/// `space` on a model row does nothing: the plan toggle is gone from the
/// window, and `plan.models` in the file is never written.
#[test]
fn space_on_a_model_row_changes_nothing() {
    let (_h, mut f) = form(Some("plan:\n  models: [opus, codex]\n"));
    keys(&mut f, &["tab"]);
    assert_eq!(f.field(), Some(Field::Model(0)));
    assert_eq!(keys(&mut f, &["space", "enter"]), [None, None]);
    assert!(!f.dirty(), "{:?}", f.patch());
    assert_eq!(f.draft.plan.models, ["opus", "codex"]);
}

/// The two `/rival` pickers follow the model rows: each cycles the five
/// models from the host's default and writes its own `skill.<host>` key.
#[test]
fn the_skill_pickers_write_their_own_keys() {
    let (_h, mut f) = form(None);
    keys(&mut f, &["tab"]);
    keys(&mut f, &["down"; 5]);
    assert_eq!(f.field(), Some(Field::Skill(SkillHost::Claude)));
    assert_eq!(f.skill_model(SkillHost::Claude).unwrap().name, "sol");
    assert_eq!(f.skill_model(SkillHost::Codex).unwrap().name, "claude");
    keys(&mut f, &["right"]);
    assert_eq!(f.skill_model(SkillHost::Claude).unwrap().name, "claude");
    assert_eq!(f.patch(), json!({"skill.claude": "claude"}));
    keys(&mut f, &["left", "left"]);
    assert_eq!(f.skill_model(SkillHost::Claude).unwrap().name, "codex");
    keys(&mut f, &["left"]);
    assert_eq!(
        f.skill_model(SkillHost::Claude).unwrap().name,
        "grok",
        "the list wraps"
    );
    keys(&mut f, &["down", "right"]);
    assert_eq!(f.field(), Some(Field::Skill(SkillHost::Codex)));
    assert_eq!(
        f.patch(),
        json!({"skill.claude": "grok", "skill.codex": "fable"})
    );
    let edits = f.edits().unwrap();
    assert_eq!(
        edits,
        [
            Edit::set("skill.claude", write::Value::Str("grok".into())),
            Edit::set("skill.codex", write::Value::Str("fable".into())),
        ]
    );
    assert!(f.draft_config().is_ok());
    keys(&mut f, &["left"]);
    assert_eq!(
        f.patch(),
        json!({"skill.claude": "grok"}),
        "the default again is no change"
    );
    // `opus` in the file is `claude`, and picking claude again is no change.
    let (_h, mut f) = form(Some("skill:\n  codex: opus\n"));
    assert_eq!(f.skill_model(SkillHost::Codex).unwrap().name, "claude");
    focus(&mut f, Field::Skill(SkillHost::Codex));
    keys(&mut f, &["right", "left"]);
    assert!(!f.dirty(), "{:?}", f.patch());
}

#[test]
fn the_auto_fix_toggle() {
    let (_h, mut f) = form(None);
    keys(&mut f, &["3", "space"]);
    assert_eq!(f.patch(), json!({"auto_fix_critical_high": true}));
}

#[test]
fn a_typed_key_waits_outside_the_draft() {
    let (_h, mut f) = form(Some(PROXY_YAML));
    focus(&mut f, Field::Key);
    keys(&mut f, &["enter"]);
    assert_eq!(
        f.edit.as_ref().unwrap().input.value(),
        "",
        "the saved key is never loaded"
    );
    typed(&mut f, "new key");
    keys(&mut f, &["enter"]);
    assert!(f.editing(), "a key with a space is refused");
    assert!(
        f.edit
            .as_ref()
            .unwrap()
            .error
            .as_deref()
            .unwrap()
            .contains("space")
    );
    f.edit.as_mut().unwrap().input.set_value("");
    f.paste("sk-draft-key-7788\n");
    assert_eq!(
        keys(&mut f, &["enter"]),
        [Some(Effect::Probe { debounce: false })]
    );
    assert!(f.dirty());
    assert_eq!(f.patch(), json!({}), "the key is not part of the config");
    assert_eq!(f.draft, f.seed.saved);
    assert!(
        !format!("{f:?}").contains("sk-draft-key-7788"),
        "Debug hides it"
    );
    assert!(matches!(f.key_view(), KeyStatus::Set { tail, .. } if tail == "7788"));
    // The draft config uses it in place of the file.
    let cfg = f.draft_config().unwrap();
    assert_eq!(
        cfg.proxy_key().unwrap().unwrap().secret(),
        "sk-draft-key-7788"
    );
    let req = f.save_request().unwrap();
    assert!(req.edits.is_empty());
    assert_eq!(req.key.unwrap().expose(), "sk-draft-key-7788");
    // u drops it.
    keys(&mut f, &["u"]);
    assert!(!f.dirty());
    assert!(f.pending_key().is_none());
}

#[test]
fn the_draft_config_keeps_the_rest_of_the_file() {
    let (_h, mut f) = form(Some(PROXY_YAML));
    focus(&mut f, Field::CodexRoute);
    keys(&mut f, &["space"]);
    let cfg = f.draft_config().unwrap();
    let user = cfg.user_config().unwrap();
    assert!(!user.proxy.codex.enabled);
    assert_eq!(user.roles["reviewer"], "Review like a staff engineer.");
    assert_eq!(
        cfg.proxy_route(ProxyProvider::Claude)
            .unwrap()
            .unwrap()
            .prefix,
        "emcd2_"
    );
    // The saved file is untouched.
    let text = std::fs::read_to_string(&f.seed.path).unwrap();
    assert_eq!(text, PROXY_YAML);
}

#[test]
fn esc_asks_before_dropping_a_draft() {
    let (_h, mut f) = form(Some(PROXY_YAML));
    assert_eq!(keys(&mut f, &["esc"]), [Some(Effect::Close)]);
    keys(&mut f, &["space", "esc"]);
    assert!(f.confirm_exit);
    assert_eq!(
        keys(&mut f, &["j", "esc"]),
        [None, None],
        "other keys wait; esc stays"
    );
    assert!(!f.confirm_exit && f.dirty());
    keys(&mut f, &["q"]);
    assert_eq!(keys(&mut f, &["n"]), [Some(Effect::Close)]);
    keys(&mut f, &["esc"]);
    assert_eq!(keys(&mut f, &["y"]), [Some(Effect::Save)]);
    assert!(f.close_after_save && f.saving);
    // While invalid, y keeps the window and says why.
    let (_h, mut f) = form(None);
    keys(&mut f, &["space", "esc", "y"]);
    assert!(!f.saving && !f.confirm_exit);
    assert_eq!(f.notice.as_ref().unwrap().kind, NoticeKind::Error);
}

#[test]
fn save_runs_once_and_lands_in_the_seed() {
    let (_h, mut f) = form(Some(PROXY_YAML));
    assert_eq!(keys(&mut f, &["s"]), [None]);
    assert_eq!(f.notice.as_ref().unwrap().text, "nothing to save");
    keys(&mut f, &["space"]);
    assert_eq!(keys(&mut f, &["s", "s"]), [Some(Effect::Save), None]);
    let req = f.save_request().unwrap();
    let saved = crate::tui::config_check::run_save(req).unwrap();
    assert!(!f.saved(&saved));
    assert!(!f.dirty() && !f.saving);
    assert!(!f.seed.saved.proxy.claude.enabled);
    assert_eq!(
        f.notice.as_ref().unwrap().text,
        "saved ~/.rival/config.yaml"
    );
}

/// While a save is in flight the draft is frozen: an edit then would be
/// dropped when the save lands and replaces the draft. Moving, checking
/// and help still work; edits work again after the save.
#[test]
fn edits_wait_while_a_save_is_in_flight() {
    let (_h, mut f) = form(Some(PROXY_YAML));
    keys(&mut f, &["space"]);
    assert_eq!(keys(&mut f, &["s"]), [Some(Effect::Save)]);
    let req = f.save_request().unwrap();
    let before = f.draft.clone();
    // Toggle, step, edit, import and undo are all refused.
    focus(&mut f, Field::CodexRoute);
    for k in ["space", "enter", "left", "right", "h", "l", "u", "d"] {
        assert_eq!(keys(&mut f, &[k]), [None], "{k}");
        assert_eq!(f.draft, before, "{k} changed the draft");
        assert_eq!(f.notice.as_ref().unwrap().text, "saving…", "{k}");
    }
    focus(&mut f, Field::Url);
    keys(&mut f, &["enter", "e"]);
    assert!(f.edit.is_none(), "no edit opens while saving");
    focus(&mut f, Field::Model(0));
    keys(&mut f, &["right"]);
    assert_eq!(f.draft, before);
    // Moving still works.
    keys(&mut f, &["tab"]);
    let saved = crate::tui::config_check::run_save(req).unwrap();
    f.saved(&saved);
    assert!(!f.dirty());
    focus(&mut f, Field::CodexRoute);
    keys(&mut f, &["space"]);
    assert!(f.dirty(), "edits work again after the save");
}

#[test]
fn a_check_runs_on_the_draft_and_drops_stale_rows() {
    let (_h, mut f) = form(Some(PROXY_YAML));
    focus(&mut f, Field::CodexRoute);
    keys(&mut f, &["space"]);
    assert_eq!(keys(&mut f, &["c"]), [Some(Effect::Check { all: false })]);
    let req = f.start_check(false).unwrap();
    let names: Vec<&str> = req.targets.iter().map(|t| t.name).collect();
    assert_eq!(names, ["codex", "sol", "claude", "fable", "china"]);
    assert!(f.check.running);
    let slot =
        |f: &ConfigForm, n: &str| f.check.slots.iter().find(|s| s.name == n).cloned().unwrap();
    assert_eq!(
        slot(&f, "codex").route,
        "direct",
        "the draft turned Codex off"
    );
    assert_eq!(slot(&f, "claude").wire, "emcd2_/claude-opus-5-5");
    assert!(f.start_check(true).is_err(), "one check at a time");
    let row = |name: &str| CheckRow {
        name: name.into(),
        ok: true,
        called: true,
        ms: 900,
        reply: "ok".into(),
        route: "direct".into(),
        wire_model: CODEX.model.into(),
        ..CheckRow::default()
    };
    f.check_row(req.run + 1, row("codex"));
    assert_eq!(f.check.done(), 0, "another run's row");
    f.check_row(req.run, row("codex"));
    assert_eq!((f.check.done(), f.check.passed()), (1, 1));
    // x cancels; the report ends it.
    keys(&mut f, &["x"]);
    assert!(f.check.cancelling && req.ctx.is_done());
    f.check_done(
        req.run,
        Report {
            proxy: ProxyLine::Off,
            rows: vec![row("codex")],
        },
        fixed_now(),
    );
    assert!(!f.check.running && !f.check.cancelling);
    assert_eq!(f.check.finished, Some(fixed_now()));
}

#[test]
fn a_check_on_an_invalid_draft_is_refused() {
    let (_h, mut f) = form(Some(PROXY_YAML));
    f.draft.efforts.insert("codex".into(), "max".into());
    f.validate();
    assert!(
        f.general_error
            .as_deref()
            .unwrap()
            .contains("invalid effort"),
        "{f:?}"
    );
    let err = f.start_check(false).unwrap_err();
    assert!(err.starts_with("check blocked: invalid effort"), "{err}");
}

#[test]
fn help_opens_with_question_mark_and_any_key_closes_it() {
    let (_h, mut f) = form(None);
    keys(&mut f, &["?"]);
    assert!(f.help);
    assert_eq!(keys(&mut f, &["s"]), [None]);
    assert!(
        !f.help && f.notice.is_none(),
        "the closing key does nothing else"
    );
}

/// The two speed pickers close the Models section; both are fast by
/// default, `←→` cycles normal → fast → ultrafast, each writes its own
/// `speed.<model>` key, only when it changed, and ultrafast warns.
#[test]
fn the_speed_pickers_write_their_own_keys() {
    let (_h, mut f) = form(None);
    focus(&mut f, Field::Speed(0));
    assert_eq!(f.section, Section::Models);
    assert_eq!(f.field().unwrap().label(), "Codex speed");
    assert_eq!(
        (f.draft.speed.codex, f.draft.speed.sol),
        (Speed::Fast, Speed::Fast)
    );
    keys(&mut f, &["right"]);
    assert_eq!(f.draft.speed.codex, Speed::Ultrafast);
    assert_eq!(f.patch(), json!({"speed.codex": "ultrafast"}));
    let notice = f.notice.clone().expect("the ultrafast warning");
    assert_eq!(notice.kind, NoticeKind::Warn);
    assert_eq!(notice.text, config::ULTRAFAST_PLAN_WARNING);
    keys(&mut f, &["right"]);
    assert_eq!(f.draft.speed.codex, Speed::Normal, "the list wraps");
    assert!(f.notice.is_none());
    keys(&mut f, &["down", "left"]);
    assert_eq!(f.field(), Some(Field::Speed(1)));
    assert_eq!(f.field().unwrap().label(), "Sol speed");
    assert_eq!(
        f.patch(),
        json!({"speed.codex": "normal", "speed.sol": "normal"})
    );
    assert_eq!(
        f.edits().unwrap(),
        [
            Edit::set("speed.codex", write::Value::Str("normal".into())),
            Edit::set("speed.sol", write::Value::Str("normal".into())),
        ]
    );
    assert!(f.draft_config().is_ok());
    keys(&mut f, &["right"]);
    assert_eq!(
        f.patch(),
        json!({"speed.codex": "normal"}),
        "fast again is no change"
    );
    assert_eq!(keys(&mut f, &["space"]), [None], "a picker does not toggle");
    keys(&mut f, &["down"]);
    assert_eq!(
        f.field(),
        Some(Field::Speed(1)),
        "the focus stops at the end"
    );
    // A 5.2 `ultrafast.sol: true` reads as ultrafast; stepping writes speed.
    let (_h, mut f) = form(Some("ultrafast:\n  sol: true\n"));
    assert_eq!(f.draft.speed.sol, Speed::Ultrafast);
    assert!(!f.dirty());
    focus(&mut f, Field::Speed(1));
    keys(&mut f, &["left"]);
    assert_eq!(f.patch(), json!({"speed.sol": "fast"}));
}

fn china_list() -> Vec<String> {
    [
        "moonshot/kimi-k3.1-pro",
        "openrouter/aion-labs/aion-2.0",
        "openrouter/moonshotai/kimi-k3.1-pro",
        "openrouter/z-ai/glm-5.3",
        "openrouter/z-ai/glm-5.3-flash",
    ]
    .map(String::from)
    .to_vec()
}

/// The provider picker waits for opencode's list, then moves the model to
/// the same name on the new provider, and back.
#[test]
fn the_china_provider_picker_keeps_the_model_name() {
    let (_h, mut f) = form(None);
    focus(&mut f, Field::ChinaProvider);
    keys(&mut f, &["right"]);
    assert_eq!(f.china_selector(), config::CHINA_DEFAULT_MODEL);
    assert_eq!(
        f.notice.as_ref().unwrap().text,
        "opencode models are loading…"
    );

    f.china_models_done(Ok(china_list()));
    keys(&mut f, &["right"]);
    assert_eq!(f.china_selector(), "openrouter/moonshotai/kimi-k3.1-pro");
    assert_eq!(
        f.patch(),
        json!({"china.model": "openrouter/moonshotai/kimi-k3.1-pro"})
    );
    keys(&mut f, &["left"]);
    assert_eq!(f.china_selector(), "moonshot/kimi-k3.1-pro");
    assert_eq!(
        f.patch(),
        json!({}),
        "the default chosen again is no change"
    );
}

/// Chinese labs come first; a provider without a listed model refuses.
#[test]
fn the_china_model_picker_lists_chinese_labs_first() {
    let (_h, mut f) = form(None);
    f.china_models_done(Ok(china_list()));
    assert_eq!(
        f.china_models("openrouter"),
        [
            "openrouter/moonshotai/kimi-k3.1-pro",
            "openrouter/z-ai/glm-5.3",
            "openrouter/z-ai/glm-5.3-flash",
            "openrouter/aion-labs/aion-2.0",
        ]
    );
    f.china_models_done(Ok(vec!["moonshot/kimi-k3.1-pro".to_string()]));
    focus(&mut f, Field::ChinaProvider);
    keys(&mut f, &["right"]);
    assert_eq!(f.china_selector(), "moonshot/kimi-k3.1-pro");
    assert!(
        f.notice
            .as_ref()
            .unwrap()
            .text
            .starts_with("opencode lists no openrouter models")
    );
}

/// `e` takes a full name, a unique part, or a full selector opencode does
/// not list (with a warning); several matches are an error.
#[test]
fn a_typed_china_model_resolves_against_the_list() {
    let (_h, mut f) = form(Some("china:\n  model: openrouter/aion-labs/aion-2.0\n"));
    f.china_models_done(Ok(china_list()));
    focus(&mut f, Field::ChinaModel);
    let set = |f: &mut ConfigForm, text: &str| {
        keys(f, &["e", "ctrl+u"]);
        typed(f, text);
        keys(f, &["enter"]);
    };
    set(&mut f, "glm-5.3");
    assert_eq!(
        f.china_selector(),
        "openrouter/z-ai/glm-5.3",
        "the exact name wins"
    );
    set(&mut f, "kimi");
    assert_eq!(f.china_selector(), "openrouter/moonshotai/kimi-k3.1-pro");
    set(&mut f, "z-ai/glm-5.3-flash");
    assert_eq!(
        f.china_selector(),
        "openrouter/z-ai/glm-5.3-flash",
        "the name as the picker shows it"
    );
    // A full selector opencode does not list stays as typed, even though a
    // listed name contains it.
    set(&mut f, "openrouter/z-ai/glm-5");
    assert_eq!(f.china_selector(), "openrouter/z-ai/glm-5");
    assert!(f.china_unlisted());
    set(&mut f, "glm");
    assert!(
        f.edit
            .as_ref()
            .unwrap()
            .error
            .as_ref()
            .unwrap()
            .starts_with("2 openrouter models match")
    );
    keys(&mut f, &["esc"]);
    set(&mut f, "hinoki/hinoki-offensive");
    assert_eq!(f.china_selector(), "hinoki/hinoki-offensive");
    assert_eq!(f.notice.as_ref().unwrap().kind, NoticeKind::Warn);
    assert!(f.china_unlisted());
}
