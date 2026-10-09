//! The writer for `~/.rival/config.yaml` and the proxy key file.
//!
//! The config file is read as a generic YAML tree, edited by key path and
//! written back in block style. Unknown keys, the key order and the text of
//! plain scalars (`yes`, `0600`) are kept; comments, anchors, tags and flow
//! style are not. The new text is validated like [`load_user_config`]
//! before it replaces the old file through a temp file and a rename. The
//! first rewrite of a file without [`HEADER`] copies it to
//! `config.yaml.bak`.
//!
//! [`load_user_config`]: super::load_user_config

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde_saphyr::granit_parser::{Event, Parser, ScalarStyle};

use super::{ConfigError, UserConfig, parse_user_config};

#[cfg(test)]
mod tests;

/// The first line of a file that rival wrote.
pub const HEADER: &str = "# written by rival config; comments are not kept";

/// The key paths `rival config set` accepts. `efforts.<model>` takes any
/// effort label; the validation checks the label.
pub const SETTABLE_KEYS: [&str; 11] = [
    "proxy.url",
    "proxy.key_file",
    "proxy.claude.enabled",
    "proxy.claude.model_prefix",
    "proxy.codex.enabled",
    "proxy.codex.model_prefix",
    "plan.models",
    "efforts.<model>",
    "security.reviewer",
    "auto_fix_critical_high",
    "claude.subscription",
];

/// A value the writer sets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Str(String),
    Bool(bool),
    List(Vec<String>),
}

/// One edit: a key path and its new value. `None` removes the key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub key: String,
    pub value: Option<Value>,
}

impl Edit {
    pub fn set(key: &str, value: Value) -> Edit {
        Edit {
            key: key.to_string(),
            value: Some(value),
        }
    }

    pub fn remove(key: &str) -> Edit {
        Edit {
            key: key.to_string(),
            value: None,
        }
    }
}

/// The type a settable key takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Str,
    Bool,
    List,
}

/// The kind of a settable key, or the error for any other key.
fn key_kind(key: &str) -> Result<Kind, ConfigError> {
    if key == "proxy.key" {
        return Err(ConfigError::new(
            "the proxy key is not stored in config.yaml — use: rival config key set",
        ));
    }
    let kind = match key {
        "proxy.url"
        | "proxy.key_file"
        | "proxy.claude.model_prefix"
        | "proxy.codex.model_prefix"
        | "security.reviewer"
        | "claude.subscription" => Kind::Str,
        "proxy.claude.enabled" | "proxy.codex.enabled" | "auto_fix_critical_high" => Kind::Bool,
        "plan.models" => Kind::List,
        _ => match key.strip_prefix("efforts.") {
            Some(label) if !label.is_empty() && !label.contains('.') => Kind::Str,
            _ => {
                return Err(ConfigError::new(format!(
                    "unknown config key {:?}; use one of: {}",
                    key,
                    SETTABLE_KEYS.join(", ")
                )));
            }
        },
    };
    Ok(kind)
}

fn bool_error(key: &str) -> ConfigError {
    ConfigError::new(format!("invalid value for {key}; use true or false"))
}

/// One `rival config set KEY VALUE`. A list is comma-separated; a bool is
/// `true` or `false`.
pub fn parse_setting(key: &str, raw: &str) -> Result<Edit, ConfigError> {
    let value = match key_kind(key)? {
        Kind::Str => Value::Str(raw.to_string()),
        Kind::Bool => match raw.trim().to_ascii_lowercase().as_str() {
            "true" => Value::Bool(true),
            "false" => Value::Bool(false),
            _ => return Err(bool_error(key)),
        },
        Kind::List => Value::List(
            raw.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect(),
        ),
    };
    Ok(Edit::set(key, value))
}

/// The edits of a JSON patch (`rival config set --json`). Objects nest
/// (`{"proxy": {"url": …}}`) or use dotted keys (`{"proxy.url": …}`);
/// `null` removes a key. A list also takes a comma-separated string. The
/// edits come in sorted key order.
pub fn edits_from_json(patch: &serde_json::Value) -> Result<Vec<Edit>, ConfigError> {
    let serde_json::Value::Object(map) = patch else {
        return Err(ConfigError::new("the patch must be a JSON object"));
    };
    let mut edits = Vec::new();
    flatten(map, "", &mut edits)?;
    Ok(edits)
}

fn flatten(
    map: &serde_json::Map<String, serde_json::Value>,
    prefix: &str,
    edits: &mut Vec<Edit>,
) -> Result<(), ConfigError> {
    use serde_json::Value as J;
    for (name, value) in map {
        let key = format!("{prefix}{name}");
        if let J::Object(inner) = value {
            flatten(inner, &format!("{key}."), edits)?;
            continue;
        }
        let kind = key_kind(&key)?;
        let value = match (kind, value) {
            (_, J::Null) => None,
            (Kind::Str, J::String(s)) => Some(Value::Str(s.clone())),
            (Kind::Str, _) => {
                return Err(ConfigError::new(format!("{key} must be a string")));
            }
            (Kind::Bool, J::Bool(b)) => Some(Value::Bool(*b)),
            (Kind::Bool, _) => return Err(bool_error(&key)),
            (Kind::List, J::String(s)) => parse_setting(&key, s)?.value,
            (Kind::List, J::Array(items)) => Some(Value::List(
                items
                    .iter()
                    .map(|i| i.as_str().map(String::from))
                    .collect::<Option<Vec<_>>>()
                    .ok_or_else(|| ConfigError::new(format!("{key} must be a list of names")))?,
            )),
            (Kind::List, _) => {
                return Err(ConfigError::new(format!("{key} must be a list of names")));
            }
        };
        edits.push(Edit { key, value });
    }
    Ok(())
}

/// Applies `edits` to the config file at `path` in one write and returns
/// the new, validated config. On any error the old file stays as it was.
pub fn apply(path: &Path, edits: &[Edit]) -> Result<UserConfig, ConfigError> {
    let shown = path.display();
    let old = match fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(ConfigError::new(format!("read {shown}: {e}"))),
    };
    let (text, user) = render(old.as_deref(), edits, path)?;

    if let Some(old) = &old
        && old.lines().next().map(str::trim_end) != Some(HEADER)
    {
        let backup = with_suffix(path, ".bak");
        fs::copy(path, &backup).map_err(|e| ConfigError::new(format!("back up {shown}: {e}")))?;
    }
    let perms = fs::metadata(path).ok().map(|m| m.permissions());
    write_atomic(path, text.as_bytes(), perms)
        .map_err(|e| ConfigError::new(format!("write {shown}: {e}")))?;
    Ok(user)
}

/// The text [`apply`] would write for `edits` on the file text `old`
/// (`None`: no file), and the config it parses to, validated as if read
/// from `path`. Nothing is written: the TUI checks a draft with it.
pub fn render(
    old: Option<&str>,
    edits: &[Edit],
    path: &Path,
) -> Result<(String, UserConfig), ConfigError> {
    let shown = path.display();
    let tree = parse_tree(old.unwrap_or(""))
        .map_err(|e| ConfigError::new(format!("parse {shown}: {e}")))?;
    let mut entries = match tree {
        None => Vec::new(),
        Some(Node::Map(entries)) => entries,
        Some(Node::Scalar(s)) if s.is_null() => Vec::new(),
        Some(_) => {
            return Err(ConfigError::new(format!(
                "cannot edit {shown}: the top level is not a map"
            )));
        }
    };
    for edit in edits {
        key_kind(&edit.key)?;
        let parts: Vec<&str> = edit.key.split('.').collect();
        set_path(&mut entries, &parts, 0, edit.value.as_ref().map(value_node))
            .map_err(|e| ConfigError::new(format!("cannot set {} in {shown}: {e}", edit.key)))?;
    }
    let mut text = format!("{HEADER}\n");
    emit_map(&mut text, &entries, 0, false)
        .map_err(|e| ConfigError::new(format!("write {shown}: {e}")))?;
    let user = parse_user_config(&text, path)?;
    Ok((text, user))
}

/// The dotted paths of every map key in the config file, at any depth
/// (`proxy`, `proxy.claude`, `proxy.claude.enabled`). Empty when the file
/// is missing.
pub fn keys_in_file(path: &Path) -> Result<HashSet<String>, ConfigError> {
    let shown = path.display();
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(HashSet::new()),
        Err(e) => return Err(ConfigError::new(format!("read {shown}: {e}"))),
    };
    let tree = parse_tree(&text).map_err(|e| ConfigError::new(format!("parse {shown}: {e}")))?;
    let mut keys = HashSet::new();
    if let Some(Node::Map(entries)) = &tree {
        collect_keys(entries, "", &mut keys);
    }
    Ok(keys)
}

fn collect_keys(entries: &[(Node, Node)], prefix: &str, keys: &mut HashSet<String>) {
    for (key, value) in entries {
        let Node::Scalar(key) = key else { continue };
        let path = format!("{prefix}{}", key.text);
        if let Node::Map(inner) = value {
            collect_keys(inner, &format!("{path}."), keys);
        }
        keys.insert(path);
    }
}

/// Writes the proxy key, trimmed, with mode 0600 (Unix), through a temp
/// file and a rename.
pub fn write_key_file(path: &Path, key: &str) -> Result<(), ConfigError> {
    let key = key.trim();
    if key.is_empty() {
        return Err(ConfigError::new("the key is empty"));
    }
    if key.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(ConfigError::new(
            "the key has a space or a line break; give one key on one line",
        ));
    }
    #[cfg(unix)]
    let perms = {
        use std::os::unix::fs::PermissionsExt as _;
        Some(fs::Permissions::from_mode(0o600))
    };
    #[cfg(not(unix))]
    let perms = None;
    write_atomic(path, format!("{key}\n").as_bytes(), perms)
        .map_err(|e| ConfigError::new(format!("write proxy key {}: {e}", path.display())))
}

/// Removes the key file. `Ok(false)` means there was none.
pub fn clear_key_file(path: &Path) -> Result<bool, ConfigError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(ConfigError::new(format!(
            "remove proxy key {}: {e}",
            path.display()
        ))),
    }
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// Writes a temp file in the target's directory, then renames it over the
/// target. `perms` are set on the temp file first.
fn write_atomic(path: &Path, data: &[u8], perms: Option<fs::Permissions>) -> std::io::Result<()> {
    let dir = match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    };
    fs::create_dir_all(dir)?;
    let mut tmp = tempfile::Builder::new()
        .prefix(".rival-write-")
        .tempfile_in(dir)?;
    tmp.write_all(data)?;
    tmp.as_file().sync_all()?;
    if let Some(perms) = perms {
        fs::set_permissions(tmp.path(), perms)?;
    }
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

// ---- the YAML tree ----

/// A scalar's text. `plain` scalars are written back as they were read;
/// others are quoted as needed.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Scalar {
    text: String,
    plain: bool,
}

impl Scalar {
    /// A plain null: empty, `~` or `null`.
    fn is_null(&self) -> bool {
        self.plain && matches!(self.text.as_str(), "" | "~" | "null" | "Null" | "NULL")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Node {
    Scalar(Scalar),
    Seq(Vec<Node>),
    Map(Vec<(Node, Node)>),
}

fn value_node(value: &Value) -> Node {
    let text = |s: &str, plain| {
        Node::Scalar(Scalar {
            text: s.to_string(),
            plain,
        })
    };
    match value {
        Value::Str(s) => text(s, false),
        Value::Bool(b) => text(if *b { "true" } else { "false" }, true),
        Value::List(items) => Node::Seq(items.iter().map(|i| text(i, false)).collect()),
    }
}

/// A collection being read, with its anchor and a key waiting for its
/// value.
struct Frame {
    node: Node,
    anchor: usize,
    key: Option<Node>,
}

/// The document as a tree; `None` for an empty file. Aliases are expanded.
fn parse_tree(text: &str) -> Result<Option<Node>, String> {
    let mut anchors: HashMap<usize, Node> = HashMap::new();
    let mut stack: Vec<Frame> = Vec::new();
    let mut root = None;
    let mut docs = 0;
    for event in Parser::new_from_str(text) {
        let (event, _) = event.map_err(|e| e.to_string())?;
        let node = match event {
            Event::DocumentStart(..) => {
                docs += 1;
                if docs > 1 {
                    return Err("the file has more than one YAML document".to_string());
                }
                continue;
            }
            Event::Alias(id) => anchors
                .get(&id)
                .cloned()
                .ok_or_else(|| "unknown alias".to_string())?,
            Event::Scalar(value, style, anchor, tag) => {
                let node = Node::Scalar(Scalar {
                    text: value.into_owned(),
                    // A tagged scalar is written as a string: tags are
                    // not kept.
                    plain: style == ScalarStyle::Plain && tag.is_none(),
                });
                if anchor != 0 {
                    anchors.insert(anchor, node.clone());
                }
                node
            }
            Event::SequenceStart(_, anchor, _) => {
                stack.push(Frame {
                    node: Node::Seq(Vec::new()),
                    anchor,
                    key: None,
                });
                continue;
            }
            Event::MappingStart(_, anchor, _) => {
                stack.push(Frame {
                    node: Node::Map(Vec::new()),
                    anchor,
                    key: None,
                });
                continue;
            }
            Event::SequenceEnd | Event::MappingEnd => {
                let frame = stack.pop().ok_or("unbalanced YAML events")?;
                if frame.anchor != 0 {
                    anchors.insert(frame.anchor, frame.node.clone());
                }
                frame.node
            }
            _ => continue,
        };
        match stack.last_mut() {
            None => root = Some(node),
            Some(frame) => match &mut frame.node {
                Node::Seq(items) => items.push(node),
                Node::Map(entries) => match frame.key.take() {
                    Some(key) => entries.push((key, node)),
                    None => frame.key = Some(node),
                },
                Node::Scalar(_) => unreachable!("frames hold collections"),
            },
        }
    }
    Ok(root)
}

/// Sets (or with `None` removes) `parts[at..]` under `entries`. Missing
/// maps are made; a null on the path becomes a map.
fn set_path(
    entries: &mut Vec<(Node, Node)>,
    parts: &[&str],
    at: usize,
    value: Option<Node>,
) -> Result<(), String> {
    let name = parts[at];
    let found = entries
        .iter()
        .position(|(k, _)| matches!(k, Node::Scalar(s) if s.text == name));
    let new_key = || {
        Node::Scalar(Scalar {
            text: name.to_string(),
            plain: false,
        })
    };
    if at + 1 == parts.len() {
        match (found, value) {
            (Some(i), Some(v)) => entries[i].1 = v,
            (None, Some(v)) => entries.push((new_key(), v)),
            (Some(i), None) => {
                entries.remove(i);
            }
            (None, None) => {}
        }
        return Ok(());
    }
    let i = match found {
        Some(i) => i,
        None if value.is_none() => return Ok(()),
        None => {
            entries.push((new_key(), Node::Map(Vec::new())));
            entries.len() - 1
        }
    };
    let child = &mut entries[i].1;
    if matches!(child, Node::Scalar(s) if s.is_null()) {
        *child = Node::Map(Vec::new());
    }
    match child {
        Node::Map(inner) => set_path(inner, parts, at + 1, value),
        _ => Err(format!("{} is not a map", parts[..=at].join("."))),
    }
}

// ---- the emitter ----

fn pad(out: &mut String, indent: usize) {
    out.extend(std::iter::repeat_n(' ', indent));
}

/// Block map entries at `indent`. With `inline_first` the first entry
/// follows a `- ` already written.
fn emit_map(
    out: &mut String,
    entries: &[(Node, Node)],
    indent: usize,
    inline_first: bool,
) -> Result<(), String> {
    for (n, (key, value)) in entries.iter().enumerate() {
        if !(inline_first && n == 0) {
            pad(out, indent);
        }
        let Node::Scalar(key) = key else {
            return Err("a map key that is not a scalar".to_string());
        };
        out.push_str(&inline_scalar(key)?);
        out.push(':');
        emit_value(out, value, indent, indent)?;
    }
    Ok(())
}

/// A block sequence at `indent`.
fn emit_seq(out: &mut String, items: &[Node], indent: usize) -> Result<(), String> {
    for item in items {
        pad(out, indent);
        out.push('-');
        match item {
            Node::Map(entries) if !entries.is_empty() => {
                out.push(' ');
                emit_map(out, entries, indent + 2, true)?;
            }
            Node::Seq(inner) if !inner.is_empty() => {
                out.push('\n');
                emit_seq(out, inner, indent + 2)?;
            }
            _ => emit_value(out, item, indent, indent + 2)?,
        }
    }
    Ok(())
}

/// The value after `key:` or `-`. A nested map goes at `indent + 2`, a
/// nested sequence at `seq_indent`.
fn emit_value(
    out: &mut String,
    value: &Node,
    indent: usize,
    seq_indent: usize,
) -> Result<(), String> {
    match value {
        Node::Scalar(s) if s.plain && s.text.is_empty() => out.push('\n'),
        Node::Scalar(s) => {
            out.push(' ');
            match literal_block(s) {
                Some((header, lines)) => {
                    out.push_str(header);
                    out.push('\n');
                    for line in lines {
                        if !line.is_empty() {
                            pad(out, indent + 2);
                            out.push_str(line);
                        }
                        out.push('\n');
                    }
                }
                None => {
                    out.push_str(&inline_scalar(s)?);
                    out.push('\n');
                }
            }
        }
        Node::Map(entries) if entries.is_empty() => out.push_str(" {}\n"),
        Node::Seq(items) if items.is_empty() => out.push_str(" []\n"),
        Node::Map(entries) => {
            out.push('\n');
            emit_map(out, entries, indent + 2, false)?;
        }
        Node::Seq(items) => {
            out.push('\n');
            emit_seq(out, items, seq_indent)?;
        }
    }
    Ok(())
}

/// A scalar on one line: plain text as read, or serde-saphyr's quoting
/// (JSON quoting when that is not one line).
fn inline_scalar(s: &Scalar) -> Result<String, String> {
    if s.plain && !s.text.contains('\n') {
        return Ok(s.text.clone());
    }
    let quoted = serde_saphyr::to_string(&s.text).map_err(|e| e.to_string())?;
    let quoted = quoted.strip_suffix('\n').unwrap_or(&quoted);
    if !quoted.contains('\n') && !quoted.starts_with(['|', '>']) {
        return Ok(quoted.to_string());
    }
    serde_json::to_string(&s.text).map_err(|e| e.to_string())
}

/// A multi-line string as a literal block: its header (`|`, `|-`, `|+`)
/// and its lines. `None` when the text cannot be one (no line break, a
/// control character, or a first line that starts with a space).
fn literal_block(s: &Scalar) -> Option<(&'static str, Vec<&str>)> {
    let text = s.text.as_str();
    if !text.contains('\n')
        || text
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        return None;
    }
    let first = text.split('\n').find(|l| !l.is_empty())?;
    if first.starts_with([' ', '\t']) {
        return None;
    }
    // Blank lines before the first text line must be empty, or they set
    // the indentation.
    if text
        .split('\n')
        .take_while(|l| l.trim().is_empty())
        .any(|l| !l.is_empty())
    {
        return None;
    }
    let (header, body) = if let Some(body) = text.strip_suffix("\n\n") {
        ("|+", &text[..body.len() + 1])
    } else if let Some(body) = text.strip_suffix('\n') {
        ("|", body)
    } else {
        ("|-", text)
    };
    Some((header, body.split('\n').collect()))
}
