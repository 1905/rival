//! The config writer and the key file writer. Every test works in its own
//! temp dir; nothing touches the real `~/.rival`.

use super::*;

use std::fs;

fn temp() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(".rival").join("config.yaml");
    (dir, path)
}

fn set(path: &Path, key: &str, value: &str) -> Result<UserConfig, ConfigError> {
    apply(path, &[parse_setting(key, value).unwrap()])
}

fn bak(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".bak");
    PathBuf::from(s)
}

/// Names in `dir` other than `keep`: left-over temp files.
fn others(dir: &Path, keep: &[&str]) -> Vec<String> {
    fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| !keep.contains(&n.as_str()))
        .collect()
}

#[test]
fn set_makes_the_missing_maps_and_the_file() {
    let (_dir, path) = temp();
    let user = set(&path, "proxy.claude.model_prefix", "emcd_/").unwrap();
    assert_eq!(user.proxy.claude.model_prefix, "emcd_");
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        format!("{HEADER}\nproxy:\n  claude:\n    model_prefix: emcd_/\n")
    );
    // A new file has no backup.
    assert!(!bak(&path).exists());
    let user = set(&path, "proxy.claude.enabled", "true").unwrap();
    assert!(user.proxy.claude.enabled);
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        format!("{HEADER}\nproxy:\n  claude:\n    model_prefix: emcd_/\n    enabled: true\n")
    );
}

#[test]
fn round_trip_keeps_unknown_keys_order_and_scalar_text() {
    let (_dir, path) = temp();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let original = "# my settings\n\
zeta: 1   # a number\n\
review:\n  models: [codex, k3]\n\
efforts:\n  codex: high\n\
alpha: yes\n\
mode: 0600\n\
quoted: 'it''s'\n\
roles:\n  bug_hunter: |\n    line one\n\n    line three\n  short: \"tab\\there\"\n\
proxy:\n  url: http://127.0.0.1:8317\n";
    fs::write(&path, original).unwrap();
    let user = set(&path, "proxy.claude.model_prefix", "emcd_").unwrap();
    assert_eq!(
        user.roles["bug_hunter"], "line one\n\nline three\n",
        "the literal block changed"
    );
    assert_eq!(user.roles["short"], "tab\there");
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        format!(
            "{HEADER}\n\
zeta: 1\n\
review:\n  models:\n  - codex\n  - k3\n\
efforts:\n  codex: high\n\
alpha: yes\n\
mode: 0600\n\
quoted: it's\n\
roles:\n  bug_hunter: |\n    line one\n\n    line three\n  short: \"tab\\there\"\n\
proxy:\n  url: http://127.0.0.1:8317\n  claude:\n    model_prefix: emcd_\n"
        )
    );
    assert_eq!(fs::read_to_string(bak(&path)).unwrap(), original);
}

#[test]
fn backup_is_made_only_on_the_first_rewrite() {
    let (dir, path) = temp();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "efforts:\n  codex: high # mine\n").unwrap();
    set(&path, "efforts.claude", "low").unwrap();
    assert_eq!(
        fs::read_to_string(bak(&path)).unwrap(),
        "efforts:\n  codex: high # mine\n"
    );
    fs::write(bak(&path), "marker").unwrap();
    set(&path, "efforts.claude", "medium").unwrap();
    assert_eq!(fs::read_to_string(bak(&path)).unwrap(), "marker");
    let rival = dir.path().join(".rival");
    assert!(others(&rival, &["config.yaml", "config.yaml.bak"]).is_empty());
}

#[test]
fn an_invalid_value_leaves_the_old_file() {
    let (dir, path) = temp();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let original = "efforts:\n  codex: high\n";
    fs::write(&path, original).unwrap();
    for (key, value, want) in [
        (
            "efforts.codex",
            "huge",
            "invalid effort \"huge\" for codex in ",
        ),
        ("proxy.url", "localhost:8317", "invalid proxy.url"),
        (
            "plan.models",
            "codex,gpt",
            "invalid plan.models entry \"gpt\"",
        ),
        ("security.reviewer", "nobody", "invalid security.reviewer"),
    ] {
        let err = set(&path, key, value).unwrap_err().to_string();
        assert!(err.contains(want), "{key}: {err}");
        // The error names the real file, not the temp file.
        assert!(err.contains(&path.display().to_string()), "{key}: {err}");
        assert_eq!(fs::read_to_string(&path).unwrap(), original, "{key}");
    }
    assert!(!bak(&path).exists());
    assert!(others(&dir.path().join(".rival"), &["config.yaml"]).is_empty());
}

#[test]
fn null_and_removed_keys() {
    let (_dir, path) = temp();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "proxy:\nplan:\n  models: [sol]\n").unwrap();
    let user = set(&path, "proxy.url", "http://h:1/v1").unwrap();
    assert_eq!(user.proxy.url, "http://h:1");
    let edits = edits_from_json(&serde_json::json!({"plan": {"models": null}})).unwrap();
    let user = apply(&path, &edits).unwrap();
    assert!(user.plan.models.is_empty());
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        format!("{HEADER}\nproxy:\n  url: http://h:1/v1\nplan: {{}}\n")
    );
    // Removing a key that is not there is a no-op.
    apply(&path, &[Edit::remove("efforts.codex")]).unwrap();
}

#[test]
fn a_non_map_on_the_path_is_an_error() {
    let (_dir, path) = temp();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "proxy: 5\n").unwrap();
    let err = set(&path, "proxy.url", "http://h").unwrap_err().to_string();
    assert!(err.contains("proxy is not a map"), "{err}");
    fs::write(&path, "- a\n").unwrap();
    let err = set(&path, "proxy.url", "http://h").unwrap_err().to_string();
    assert!(err.contains("not a map"), "{err}");
    fs::write(&path, "a: 1\n---\nb: 2\n").unwrap();
    let err = set(&path, "proxy.url", "http://h").unwrap_err().to_string();
    assert!(err.contains("more than one"), "{err}");
}

#[test]
fn values_that_look_like_other_types_are_quoted() {
    let (_dir, path) = temp();
    let edits = edits_from_json(&serde_json::json!({
        "claude.subscription": "true",
        "proxy.codex.model_prefix": "",
        "proxy.key_file": "~/k #1",
    }))
    .unwrap();
    let user = apply(&path, &edits).unwrap();
    assert_eq!(user.claude.subscription, "true");
    assert_eq!(user.proxy.codex.model_prefix, "");
    assert_eq!(user.proxy.key_file, "~/k #1");
}

#[test]
fn parse_setting_checks_keys_and_types() {
    let e = parse_setting("proxy.codex.enabled", "TRUE").unwrap();
    assert_eq!(e, Edit::set("proxy.codex.enabled", Value::Bool(true)));
    let e = parse_setting("plan.models", " codex, fable ,").unwrap();
    assert_eq!(
        e,
        Edit::set(
            "plan.models",
            Value::List(vec!["codex".into(), "fable".into()])
        )
    );
    let e = parse_setting("efforts.sol", "xhigh").unwrap();
    assert_eq!(e, Edit::set("efforts.sol", Value::Str("xhigh".into())));
    for (key, value, want) in [
        ("proxy.key", "sk-x", "rival config key set"),
        ("proxy.nope", "x", "unknown config key \"proxy.nope\""),
        ("efforts", "x", "unknown config key"),
        ("efforts.a.b", "x", "unknown config key"),
        ("auto_fix_critical_high", "maybe", "use true or false"),
    ] {
        let err = parse_setting(key, value).unwrap_err().to_string();
        assert!(err.contains(want), "{key}: {err}");
        assert!(!err.contains("sk-x"), "{err}");
    }
}

#[test]
fn edits_from_json_flattens_nested_and_dotted_keys() {
    let edits = edits_from_json(&serde_json::json!({
        "proxy": {"url": "http://h", "claude": {"enabled": true}},
        "plan.models": ["codex", "sol"],
        "efforts": {"fable": "low"},
        "auto_fix_critical_high": null,
    }))
    .unwrap();
    // Keys apply in sorted order.
    assert_eq!(
        edits,
        [
            Edit::remove("auto_fix_critical_high"),
            Edit::set("efforts.fable", Value::Str("low".into())),
            Edit::set(
                "plan.models",
                Value::List(vec!["codex".into(), "sol".into()])
            ),
            Edit::set("proxy.claude.enabled", Value::Bool(true)),
            Edit::set("proxy.url", Value::Str("http://h".into())),
        ]
    );
    for (patch, want) in [
        (serde_json::json!([1]), "a JSON object"),
        (
            serde_json::json!({"proxy": {"key": "sk-x"}}),
            "rival config key set",
        ),
        (
            serde_json::json!({"proxy.claude.enabled": "yes"}),
            "use true or false",
        ),
        (serde_json::json!({"proxy.url": 5}), "a string"),
        (serde_json::json!({"plan.models": [1]}), "a list of names"),
    ] {
        let err = edits_from_json(&patch).unwrap_err().to_string();
        assert!(err.contains(want), "{patch}: {err}");
        assert!(!err.contains("sk-x"), "{err}");
    }
}

#[test]
fn key_file_is_owner_only_and_replaced_atomically() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sub").join("proxy.key");
    write_key_file(&path, "  sk-test-1234\n").unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "sk-test-1234\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        // An old file open to others is replaced by an owner-only one.
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        write_key_file(&path, "sk-test-5678").unwrap();
        assert_eq!(mode(&path), 0o600);
    }
    assert_eq!(fs::read_to_string(&path).unwrap(), "sk-test-5678\n");
    assert!(others(&dir.path().join("sub"), &["proxy.key"]).is_empty());
    for bad in ["", " \n", "two words", "a\nb"] {
        let err = write_key_file(&path, bad).unwrap_err().to_string();
        assert!(err.contains("key"), "{bad:?}: {err}");
    }
    assert_eq!(fs::read_to_string(&path).unwrap(), "sk-test-5678\n");
}

#[test]
fn clear_key_file_removes_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("proxy.key");
    write_key_file(&path, "sk-test-1234").unwrap();
    assert_eq!(clear_key_file(&path), Ok(true));
    assert!(!path.exists());
    assert_eq!(clear_key_file(&path), Ok(false));
}

#[test]
fn emitted_tree_reads_back_the_same() {
    let doc = "a: [{x: 1, y: [p, q]}, [r, [s]], {}, [], '', ~]\n\
b: &base {k: v, 'n m': \"two\\nlines\\n\"}\n\
c: *base\n\
d: \"  lead\\nspace\"\n\
e: \"keep\\n\\n\"\n\
f: \"no end\\nline\"\n\
'g: h': i\n";
    let tree = parse_tree(doc).unwrap().unwrap();
    let Node::Map(entries) = &tree else {
        panic!("{tree:?}")
    };
    let mut text = String::new();
    emit_map(&mut text, entries, 0, false).unwrap();
    let back = parse_tree(&text).unwrap().unwrap();
    // Quoting can change, the text cannot.
    fn texts(n: &Node, out: &mut Vec<String>) {
        match n {
            Node::Scalar(s) => out.push(s.text.clone()),
            Node::Seq(items) => {
                out.push("[".into());
                items.iter().for_each(|i| texts(i, out));
                out.push("]".into());
            }
            Node::Map(entries) => {
                out.push("{".into());
                for (k, v) in entries {
                    texts(k, out);
                    texts(v, out);
                }
                out.push("}".into());
            }
        }
    }
    let (mut want, mut got) = (Vec::new(), Vec::new());
    texts(&tree, &mut want);
    texts(&back, &mut got);
    assert_eq!(got, want, "{text}");
}

#[test]
fn render_builds_the_new_text_without_writing() {
    let (_dir, path) = temp();
    let old = "roles:\n  reviewer: be brief\nproxy:\n  url: http://127.0.0.1:8317/v1\n";
    let edits = [
        parse_setting("proxy.claude.enabled", "true").unwrap(),
        parse_setting("proxy.claude.model_prefix", "emcd_").unwrap(),
    ];
    let (text, user) = render(Some(old), &edits, &path).unwrap();
    assert_eq!(
        text,
        format!(
            "{HEADER}\nroles:\n  reviewer: be brief\nproxy:\n  url: http://127.0.0.1:8317/v1\n  \
             claude:\n    enabled: true\n    model_prefix: emcd_\n"
        )
    );
    assert_eq!(user.proxy.url, "http://127.0.0.1:8317");
    assert!(user.proxy.claude.enabled);
    assert_eq!(user.roles["reviewer"], "be brief");
    assert!(!path.exists(), "render never writes");
    // An invalid result is the parse error, naming the path.
    let bad = [parse_setting("proxy.url", "ftp://x").unwrap()];
    let err = render(None, &bad, &path).unwrap_err().to_string();
    assert!(
        err.starts_with("invalid proxy.url \"ftp://x\" in "),
        "{err}"
    );
}
