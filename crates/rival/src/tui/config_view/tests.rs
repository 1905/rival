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
