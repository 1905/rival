//! The config window's jobs and seed, in temp homes with a fake proxy.

use super::*;
use crate::tui::testkit::{PROXY_YAML, TEST_KEY, config_seed, fake_check, fake_models, harness};

#[test]
fn a_secret_never_shows_in_debug() {
    let key = Secret::new("  sk-secret-value-9876\n");
    assert_eq!(key.expose(), "sk-secret-value-9876");
    assert_eq!(key.tail(), "9876");
    assert_eq!(format!("{key:?}"), "Secret(<hidden>)");
    assert_eq!(Secret::new("short").tail(), "");
}

#[test]
fn paths_under_home_show_with_a_tilde() {
    let home = std::path::Path::new("/home/u");
    assert_eq!(
        shorten(&home.join(".rival/config.yaml"), "/home/u"),
        "~/.rival/config.yaml"
    );
    assert_eq!(shorten(home, "/home/u"), "~");
    assert_eq!(shorten(std::path::Path::new("/etc/x"), "/home/u"), "/etc/x");
    assert_eq!(shorten(std::path::Path::new("/etc/x"), ""), "/etc/x");
}

#[test]
fn the_seed_reads_the_file_the_key_and_the_env() {
    let h = harness();
    let seed = config_seed(&h, Some(PROXY_YAML), Some(TEST_KEY));
    assert_eq!(seed.text.as_deref(), Some(PROXY_YAML));
    assert_eq!(seed.saved.proxy.claude.model_prefix, "emcd2_");
    assert_eq!(seed.key_path_shown, "~/.rival/proxy.key");
    assert_eq!(
        seed.key,
        KeyStatus::Set {
            tail: "a91f".into(),
            env: false,
            mode: if cfg!(unix) { Some(0o600) } else { None },
        }
    );
    // No file and no key.
    let h = harness();
    let seed = config_seed(&h, None, None);
    assert_eq!(seed.text, None);
    assert_eq!(seed.key, KeyStatus::Missing);
    // The key from the environment, and the env switches.
    let env = [
        (rival_core::paths::HOME_VAR, h.home.path().to_str().unwrap()),
        ("RIVAL_PROXY_KEY", "env-key-12345678"),
        ("RIVAL_PROXY", "off"),
        ("ANTHROPIC_BASE_URL", " http://localhost:4000 "),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    let cfg = Config::new(h.paths().clone(), env, None);
    let seed = ConfigSeed::load(&cfg);
    assert_eq!(
        seed.key,
        KeyStatus::Set {
            tail: "5678".into(),
            env: true,
            mode: None
        }
    );
    assert!(seed.proxy_off_env);
    assert_eq!(seed.base_url_env, "http://localhost:4000");
}

#[cfg(unix)]
#[test]
fn a_key_file_others_can_read_is_an_error() {
    use std::os::unix::fs::PermissionsExt as _;
    let h = harness();
    let seed = config_seed(&h, None, Some(TEST_KEY));
    std::fs::set_permissions(&seed.key_path, std::fs::Permissions::from_mode(0o644)).unwrap();
    let seed = config_seed(&h, None, None);
    let KeyStatus::Error(e) = seed.key else {
        panic!("{:?}", seed.key);
    };
    assert!(e.contains("open to other users"), "{e}");
}

#[test]
fn a_probe_needs_a_url_and_a_key() {
    let h = harness();
    let no_url = config_seed(&h, None, Some(TEST_KEY));
    let res = run_probe(
        ProbeRequest {
            seq: 1,
            cfg: no_url.cfg,
        },
        fake_models,
    );
    assert_eq!(
        res,
        ProbeResult {
            seq: 1,
            result: Err("no URL".into())
        }
    );
    let h = harness();
    let no_key = config_seed(&h, Some(PROXY_YAML), None);
    let res = run_probe(
        ProbeRequest {
            seq: 2,
            cfg: no_key.cfg.clone(),
        },
        fake_models,
    );
    assert_eq!(res.result, Err(PROXY_KEY_MISSING.to_string()));
    let res = run_probe(
        ProbeRequest {
            seq: 3,
            cfg: no_key.cfg.with_draft_proxy_key("wrong-key-0000"),
        },
        fake_models,
    );
    assert_eq!(
        res.result,
        Err("proxy rejected the key (401) — run rival config key set".into())
    );
    let ok = config_seed(&h, Some(PROXY_YAML), Some(TEST_KEY));
    let res = run_probe(
        ProbeRequest {
            seq: 4,
            cfg: ok.cfg,
        },
        fake_models,
    );
    assert_eq!(res.result.unwrap().len(), 6);
}

#[test]
fn a_check_job_emits_each_row_then_the_report() {
    let h = harness();
    let seed = config_seed(&h, Some(PROXY_YAML), Some(TEST_KEY));
    let targets = crate::check::targets(&seed.cfg, &[]).unwrap();
    let (ctx, _cancel) = Context::background().with_cancel();
    let seen = std::sync::Mutex::new(Vec::new());
    let done = run_check(
        CheckRequest {
            run: 7,
            cfg: seed.cfg,
            targets,
            ctx,
        },
        fake_check,
        &|msg| seen.lock().unwrap().push(msg),
    );
    let seen = seen.into_inner().unwrap();
    assert_eq!(seen.len(), 5);
    assert!(
        seen.iter()
            .all(|m| matches!(m, Msg::CheckRow { run: 7, .. }))
    );
    let Msg::CheckDone { run: 7, report } = done else {
        panic!("{done:?}");
    };
    assert_eq!(report.rows[2].wire_model, "emcd2_/claude-opus-5-5");
}

#[test]
fn save_writes_the_config_then_the_key() {
    let h = harness();
    let seed = config_seed(&h, Some(PROXY_YAML), None);
    let edits = write::edits_from_json(&serde_json::json!({"proxy.codex.enabled": false})).unwrap();
    let saved = run_save(SaveRequest {
        path: seed.path.clone(),
        edits,
        key: Some(Secret::new("sk-saved-4321")),
        key_path: seed.key_path.clone(),
    })
    .unwrap();
    assert!(!saved.user.as_ref().unwrap().proxy.codex.enabled);
    let text = std::fs::read_to_string(&seed.path).unwrap();
    assert_eq!(saved.text.as_deref(), Some(text.as_str()));
    assert_eq!(
        std::fs::read_to_string(&seed.key_path).unwrap(),
        "sk-saved-4321\n"
    );
    // A key alone leaves the config file as it is.
    let saved = run_save(SaveRequest {
        path: seed.path.clone(),
        edits: Vec::new(),
        key: Some(Secret::new("sk-other-8765")),
        key_path: seed.key_path.clone(),
    })
    .unwrap();
    assert_eq!(saved.user, None);
    assert_eq!(std::fs::read_to_string(&seed.path).unwrap(), text);
    // An invalid edit changes nothing.
    let bad = write::edits_from_json(&serde_json::json!({"proxy.url": "ftp://x"})).unwrap();
    let err = run_save(SaveRequest {
        path: seed.path.clone(),
        edits: bad,
        key: None,
        key_path: seed.key_path.clone(),
    })
    .unwrap_err();
    assert!(err.contains("invalid proxy.url"), "{err}");
    assert_eq!(std::fs::read_to_string(&seed.path).unwrap(), text);
}

#[test]
fn the_seed_follows_a_save() {
    let h = harness();
    let mut seed = config_seed(&h, Some(PROXY_YAML), None);
    let mut user = seed.saved.clone();
    user.proxy.url = "http://127.0.0.1:9999".into();
    seed.saved(&Saved {
        user: Some(user),
        text: Some("x".into()),
        key: Some(Secret::new("sk-after-save-1111")),
    });
    assert_eq!(seed.cfg.proxy_url().unwrap(), "http://127.0.0.1:9999");
    assert_eq!(seed.text.as_deref(), Some("x"));
    assert!(matches!(&seed.key, KeyStatus::Set { tail, env: false, .. } if tail == "1111"));
}
