//! View pieces that the golden frames do not pin on their own: the key
//! bar's priorities and the check row layout at both widths.

use super::*;
use crate::tui::config_form::ConfigForm;
use crate::tui::styles::STYLES;
use crate::tui::testkit::{PROXY_YAML, TEST_KEY, config_seed, harness};

fn form() -> (crate::tui::testkit::Harness, ConfigForm) {
    let h = harness();
    let seed = config_seed(&h, Some(PROXY_YAML), Some(TEST_KEY));
    (h, ConfigForm::new(seed))
}

#[test]
fn the_key_bar_drops_minor_keys_first_and_keeps_esc_and_save() {
    let (_h, f) = form();
    let wide = footer_line(&f, 200, &STYLES).to_string();
    assert_eq!(
        wide,
        "tab section  ↑↓ field  space toggle  c check  a all  s save  ? help  esc back"
    );
    let narrow = footer_line(&f, 40, &STYLES).to_string();
    assert!(narrow.ends_with("s save  ? help  esc back"), "{narrow}");
    assert!(narrow.chars().count() <= 40, "{narrow}");
    for w in [0, 5, 12, 30] {
        assert!(footer_line(&f, w, &STYLES).width() <= w, "w={w}");
    }
}

#[test]
fn the_edit_bar_is_enter_and_esc() {
    let (_h, mut f) = form();
    f.apply("down", &crate::tui::testkit::press("down"));
    f.apply("down", &crate::tui::testkit::press("down"));
    f.apply("enter", &crate::tui::testkit::press("enter"));
    assert_eq!(
        footer_line(&f, 80, &STYLES).to_string(),
        "enter save field  esc cancel"
    );
}

#[test]
fn check_rows_put_long_messages_under_narrow_rows() {
    let (_h, mut f) = form();
    f.start_check(false).unwrap();
    let run = f.check.run;
    f.check_row(
        run,
        CheckRow {
            name: "sol".into(),
            route: "proxy".into(),
            wire_model: "gpt-6.1-sol".into(),
            error: "proxy does not serve gpt-6.1-sol".into(),
            ..CheckRow::default()
        },
    );
    let v = ViewCtx {
        styles: &STYLES,
        spin: "⠋",
    };
    let wide: Vec<String> = check_rows(&f, 120, 2, false, &v)
        .iter()
        .map(ToString::to_string)
        .collect();
    let sol = wide.iter().find(|l| l.contains("sol ")).unwrap();
    assert!(sol.contains("—  proxy does not serve gpt-6.1-sol"), "{sol}");
    let narrow: Vec<String> = check_rows(&f, 74, 2, false, &v)
        .iter()
        .map(ToString::to_string)
        .collect();
    let i = narrow.iter().position(|l| l.contains("✗ sol")).unwrap();
    assert!(!narrow[i].contains("proxy does"), "{:?}", narrow[i]);
    assert_eq!(narrow[i + 1], "    proxy does not serve gpt-6.1-sol");
    assert!(narrow.iter().all(|l| crate::tui::text::width(l) <= 74));
}

/// A pass that Codex ran at the standard tier says so in the TUI rows, as
/// the text report does; a model with the switch on says `[ultrafast]`.
#[test]
fn check_rows_show_the_ultrafast_tag() {
    let (_h, mut f) = form();
    f.start_check(false).unwrap();
    let run = f.check.run;
    for (name, omitted) in [("sol", true), ("codex", false)] {
        f.check_row(
            run,
            CheckRow {
                name: name.into(),
                route: "direct".into(),
                wire_model: name.into(),
                ok: true,
                reply: "ok".into(),
                called: true,
                ultrafast: true,
                tier_omitted: omitted,
                ..CheckRow::default()
            },
        );
    }
    let v = ViewCtx {
        styles: &STYLES,
        spin: "⠋",
    };
    let lines: Vec<String> = check_rows(&f, 120, 2, false, &v)
        .iter()
        .map(ToString::to_string)
        .collect();
    let line = |name: &str| {
        lines
            .iter()
            .find(|l| l.contains(&format!("{name} ")))
            .unwrap()
            .clone()
    };
    assert!(
        line("sol").contains("ok  [ultrafast not applied]"),
        "{lines:?}"
    );
    assert!(line("codex").contains("ok  [ultrafast]"), "{lines:?}");
}

/// A focused `/rival` picker shows its arrows, the model's display name and
/// runtime, and its help; the key bar offers `←→ choose`.
#[test]
fn the_skill_picker_renders_its_model_and_help() {
    let (_h, mut f) = form();
    f.section = Section::Models;
    f.focus = Section::Models
        .fields()
        .iter()
        .position(|x| *x == Field::Skill(SkillHost::Codex))
        .unwrap();
    f.apply("right", &crate::tui::testkit::press("right"));
    let v = ViewCtx {
        styles: &STYLES,
        spin: "⠋",
    };
    let mut out = Vec::new();
    model_lines(&f, 120, &v, &mut out);
    let lines: Vec<String> = out.iter().map(|b| b.line.to_string()).collect();
    let row = lines
        .iter()
        .position(|l| l.contains("in Codex"))
        .unwrap_or_else(|| panic!("{lines:#?}"));
    assert_eq!(
        lines[row].trim_end(),
        "▸ in Codex            ‹ fable › ←→   Fable 5.1 · Claude Code"
    );
    assert!(out[row].focus);
    assert!(
        lines[row + 1].contains("the model /rival runs in Codex (default: claude)"),
        "{lines:#?}"
    );
    assert!(
        lines.iter().any(|l| l.trim_end() == "/RIVAL RUNS"),
        "{lines:#?}"
    );
    assert!(lines.iter().all(|l| !l.contains("PLAN")), "{lines:#?}");
    assert!(
        footer_line(&f, 200, &STYLES)
            .to_string()
            .contains("←→ choose"),
    );
}
