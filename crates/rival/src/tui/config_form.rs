//! The config window's state: the saved config, the draft, the focused
//! field, a field edit, the online dot and the check rows.
//!
//! Pure: keys go in through [`ConfigForm::apply`] and come out as an
//! [`Effect`] the model turns into jobs and timers. The draft is a
//! [`UserConfig`]; [`ConfigForm::patch`] is its difference from the saved
//! config, and [`ConfigForm::draft_config`] runs that patch through the
//! config writer's text and parser, as `config check --config-stdin` reads a
//! draft. A typed proxy key never enters the draft: it waits in its own
//! slot until `s` writes it with the key file writer.

use chrono::{DateTime, FixedOffset};
use crossterm::event::KeyEvent;
use serde_json::{Map, Value as Json, json};

use rival_core::cancel::{CancelFunc, Context};
use rival_core::config::write::{self, Edit};
use rival_core::config::{
    self, CLAUDE_LABEL, CODEX_LABEL, Config, FABLE_LABEL, GROK_LABEL, K3_LABEL, ProxyProvider,
    SECURITY_REVIEWER_GROK, SECURITY_REVIEWER_K3, SOL_LABEL, UserConfig, VALID_EFFORTS,
};
use rival_core::proxy;

use super::config_check::{
    CheckRequest, ConfigSeed, KeyStatus, ProbeRequest, SaveRequest, Saved, Secret, env_key_wins,
};
use super::input::TextInput;
use crate::check::{self, CheckRow, ProxyLine, Report};

#[cfg(test)]
mod tests;

/// The pause in typing before the URL or key is tried against the proxy.
pub const PROBE_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(600);

/// The window's sections, in tab order.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Section {
    #[default]
    Proxy,
    Models,
    Review,
    Check,
}

impl Section {
    pub const ALL: [Section; 4] = [
        Section::Proxy,
        Section::Models,
        Section::Review,
        Section::Check,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Section::Proxy => "Proxy",
            Section::Models => "Models",
            Section::Review => "Review",
            Section::Check => "Check",
        }
    }

    fn index(self) -> usize {
        Section::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }

    fn step(self, delta: isize) -> Section {
        let n = Section::ALL.len() as isize;
        Section::ALL[(self.index() as isize + delta).rem_euclid(n) as usize]
    }

    /// The fields of the section, top to bottom.
    pub fn fields(self) -> Vec<Field> {
        match self {
            Section::Proxy => vec![
                Field::ClaudeRoute,
                Field::CodexRoute,
                Field::Url,
                Field::Key,
                Field::ClaudePrefix,
                Field::CodexPrefix,
            ],
            Section::Models => (0..MODELS.len()).map(Field::Model).collect(),
            Section::Review => vec![Field::Reviewer, Field::AutoFix],
            Section::Check => Vec::new(),
        }
    }
}

/// One field of the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Field {
    ClaudeRoute,
    CodexRoute,
    Url,
    Key,
    ClaudePrefix,
    CodexPrefix,
    /// A row of [`MODELS`]: its effort picker and plan default.
    Model(usize),
    Reviewer,
    AutoFix,
}

impl Field {
    pub fn label(self) -> &'static str {
        match self {
            Field::ClaudeRoute => "Claude via proxy",
            Field::CodexRoute => "Codex via proxy",
            Field::Url => "URL",
            Field::Key => "Key",
            Field::ClaudePrefix => "Claude prefix",
            Field::CodexPrefix => "Codex prefix",
            Field::Model(i) => MODELS.get(i).map_or("", |m| m.name),
            Field::Reviewer => "Security reviewer",
            Field::AutoFix => "Auto-fix crit/high",
        }
    }

    pub fn is_toggle(self) -> bool {
        matches!(
            self,
            Field::ClaudeRoute | Field::CodexRoute | Field::AutoFix
        )
    }

    pub fn is_picker(self) -> bool {
        matches!(
            self,
            Field::ClaudePrefix | Field::CodexPrefix | Field::Reviewer | Field::Model(_)
        )
    }

    /// Whether `enter` opens a text edit.
    pub fn is_text(self) -> bool {
        matches!(
            self,
            Field::Url | Field::Key | Field::ClaudePrefix | Field::CodexPrefix
        )
    }

    fn provider(self) -> Option<ProxyProvider> {
        match self {
            Field::ClaudeRoute | Field::ClaudePrefix => Some(ProxyProvider::Claude),
            Field::CodexRoute | Field::CodexPrefix => Some(ProxyProvider::Codex),
            _ => None,
        }
    }
}

/// A model as the window shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelInfo {
    /// The row name and the `-m` name.
    pub name: &'static str,
    /// The `efforts.<label>` key.
    pub effort_label: &'static str,
    pub display: &'static str,
    pub runtime: &'static str,
    pub provider: Option<ProxyProvider>,
    pub model: &'static str,
    /// The efforts `←→` steps through.
    pub ladder: &'static [&'static str],
    /// Whether `plan.models` takes it.
    pub plan: bool,
    /// Any one of them runs the model.
    pub binaries: &'static [&'static str],
}

const GROK_LADDER: [&str; 3] = ["low", "medium", "high"];
const K3_LADDER: [&str; 1] = ["max"];

/// The window's models, in row order.
pub const MODELS: [ModelInfo; 6] = [
    ModelInfo {
        name: CODEX_LABEL,
        effort_label: CODEX_LABEL,
        display: "Codex",
        runtime: "Codex CLI",
        provider: Some(ProxyProvider::Codex),
        model: config::CODEX_MODEL,
        ladder: &VALID_EFFORTS,
        plan: true,
        binaries: &["codex"],
    },
    ModelInfo {
        name: SOL_LABEL,
        effort_label: SOL_LABEL,
        display: "Sol 6.1",
        runtime: "Codex CLI",
        provider: Some(ProxyProvider::Codex),
        model: config::SOL_MODEL,
        ladder: &VALID_EFFORTS,
        plan: true,
        binaries: &["codex"],
    },
    ModelInfo {
        name: CLAUDE_LABEL,
        effort_label: CLAUDE_LABEL,
        display: "Opus 5.5",
        runtime: "Claude Code",
        provider: Some(ProxyProvider::Claude),
        model: config::CLAUDE_MODEL,
        ladder: &VALID_EFFORTS,
        plan: true,
        binaries: &["claude", "docker"],
    },
    ModelInfo {
        name: FABLE_LABEL,
        effort_label: FABLE_LABEL,
        display: "Fable 5.1",
        runtime: "Claude Code",
        provider: Some(ProxyProvider::Claude),
        model: config::FABLE_MODEL,
        ladder: &VALID_EFFORTS,
        plan: true,
        binaries: &["claude", "docker"],
    },
    ModelInfo {
        name: config::K3_COMMAND_NAME,
        effort_label: K3_LABEL,
        display: "Kimi K3",
        runtime: "OpenCode",
        provider: None,
        model: config::KIMI_MODEL,
        ladder: &K3_LADDER,
        plan: false,
        binaries: &["opencode"],
    },
    ModelInfo {
        name: GROK_LABEL,
        effort_label: GROK_LABEL,
        display: "Grok 4.6",
        runtime: "grok CLI",
        provider: None,
        model: config::GROK_MODEL,
        ladder: &GROK_LADDER,
        plan: false,
        binaries: &["grok"],
    },
];

/// The security reviewers the picker offers, with what runs them.
pub const REVIEWERS: [(&str, &str); 2] = [
    (SECURITY_REVIEWER_K3, "Kimi K3 through OpenCode"),
    (SECURITY_REVIEWER_GROK, "Grok 4.6 on OpenRouter"),
];

/// What the model must do after a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// Ask the proxy for its models: after [`PROBE_DEBOUNCE`] when
    /// `debounce`, else now.
    Probe { debounce: bool },
    /// Start a check: the default models, or every model.
    Check { all: bool },
    /// Write the draft.
    Save,
    /// Leave the window.
    Close,
}

/// A text edit of one field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldEdit {
    pub field: Field,
    pub input: TextInput,
    /// Why `enter` did not take the text.
    pub error: Option<String>,
}

/// The line under the key bar after an action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub text: String,
    pub kind: NoticeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeKind {
    Info,
    Ok,
    Error,
}

/// The proxy behind the online dot.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ProbeState {
    /// Nothing to ask: no URL, or no key.
    #[default]
    Idle,
    /// A call is due or running.
    Waiting,
    Online(usize),
    Offline(String),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Probe {
    /// The newest probe; older results are dropped.
    pub seq: u64,
    pub state: ProbeState,
    /// The ids of the last good answer, for the prefix pickers.
    pub ids: Vec<String>,
}

/// One row of the check panel: known before the check runs, its result once
/// it finished.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckSlot {
    pub name: String,
    pub route: String,
    pub wire: String,
    pub row: Option<CheckRow>,
}

/// The check panel.
#[derive(Debug, Clone, Default)]
pub struct CheckPanel {
    /// The newest run; rows of older runs are dropped.
    pub run: u64,
    pub slots: Vec<CheckSlot>,
    pub running: bool,
    pub cancelling: bool,
    /// When the last run ended, on the model's clock.
    pub finished: Option<DateTime<FixedOffset>>,
    pub proxy: Option<ProxyLine>,
    cancel: Option<CancelFunc>,
}

impl CheckPanel {
    pub fn passed(&self) -> usize {
        self.slots
            .iter()
            .filter(|s| s.row.as_ref().is_some_and(|r| r.ok))
            .count()
    }

    pub fn done(&self) -> usize {
        self.slots.iter().filter(|s| s.row.is_some()).count()
    }

    /// Stops a running check; its rows come back as failed.
    pub fn cancel(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel.cancel();
            self.cancelling = true;
        }
    }
}

/// The config window.
#[derive(Debug, Clone)]
pub struct ConfigForm {
    pub seed: ConfigSeed,
    pub draft: UserConfig,
    pub section: Section,
    /// The focused field's index in the section's fields.
    pub focus: usize,
    /// The first body line shown on a section without fields (Check).
    pub scroll: usize,
    pub edit: Option<FieldEdit>,
    /// A typed key; written with the draft.
    pending_key: Option<Secret>,
    /// The draft's invalid fields, in field order.
    pub errors: Vec<(Field, String)>,
    /// A draft the parser refuses for a reason no field shows.
    pub general_error: Option<String>,
    pub notice: Option<Notice>,
    /// The `save changes? y/n/esc` bar.
    pub confirm_exit: bool,
    /// The `?` overlay.
    pub help: bool,
    pub probe: Probe,
    pub check: CheckPanel,
    /// A save job is out.
    pub saving: bool,
    /// Leave once the save lands (`y` in the confirm bar).
    pub close_after_save: bool,
}

impl ConfigForm {
    pub fn new(seed: ConfigSeed) -> ConfigForm {
        let draft = seed.saved.clone();
        let mut form = ConfigForm {
            seed,
            draft,
            section: Section::Proxy,
            focus: 0,
            scroll: 0,
            edit: None,
            pending_key: None,
            errors: Vec::new(),
            general_error: None,
            notice: None,
            confirm_exit: false,
            help: false,
            probe: Probe::default(),
            check: CheckPanel::default(),
            saving: false,
            close_after_save: false,
        };
        form.validate();
        form
    }

    /// The focused field, if the section has fields.
    pub fn field(&self) -> Option<Field> {
        self.section.fields().get(self.focus).copied()
    }

    pub fn editing(&self) -> bool {
        self.edit.is_some()
    }

    /// Whether the draft or a typed key differs from what is saved.
    pub fn dirty(&self) -> bool {
        self.pending_key.is_some() || !self.patch_map().is_empty()
    }

    pub fn pending_key(&self) -> Option<&Secret> {
        self.pending_key.as_ref()
    }

    /// The error of `field`, if it is invalid.
    pub fn error(&self, field: Field) -> Option<&str> {
        self.errors
            .iter()
            .find(|(f, _)| *f == field)
            .map(|(_, e)| e.as_str())
    }

    // --- keys ---------------------------------------------------------------

    /// Applies one key. `ev` is the raw event, for the text edit.
    pub fn apply(&mut self, key: &str, ev: &KeyEvent) -> Option<Effect> {
        if self.help {
            self.help = false;
            return None;
        }
        if self.confirm_exit {
            return self.confirm_key(key);
        }
        if self.edit.is_some() {
            return self.edit_key(key, ev);
        }
        self.notice = None;
        // The landing save replaces the draft, so an edit made meanwhile
        // would be lost: the draft is frozen until then.
        if self.saving
            && matches!(
                key,
                "enter" | "space" | "left" | "h" | "right" | "l" | "e" | "d" | "u"
            )
        {
            self.info("saving…");
            return None;
        }
        match key {
            "tab" => self.set_section(self.section.step(1)),
            "shift+tab" => self.set_section(self.section.step(-1)),
            "1" | "2" | "3" | "4" => {
                let i = usize::from(key.as_bytes()[0] - b'1');
                self.set_section(Section::ALL[i]);
            }
            "up" | "k" => self.move_focus(-1),
            "down" | "j" => self.move_focus(1),
            "enter" => return self.activate(),
            "space" => return self.toggle(),
            "left" | "h" => return self.step(-1),
            "right" | "l" => return self.step(1),
            "e" => {
                if matches!(self.field(), Some(Field::ClaudePrefix | Field::CodexPrefix)) {
                    self.open_edit();
                }
            }
            "d" => return self.import_base_url(),
            "c" => return Some(Effect::Check { all: false }),
            "a" => return Some(Effect::Check { all: true }),
            "x" => {
                if self.check.running {
                    self.check.cancel();
                    self.info("cancelling the check");
                }
            }
            "s" => return self.save(),
            "u" => return self.revert(),
            "?" => self.help = true,
            "esc" | "q" => {
                if self.dirty() {
                    self.confirm_exit = true;
                } else {
                    return Some(Effect::Close);
                }
            }
            _ => {}
        }
        None
    }

    /// Pasted text: only a field edit takes it.
    pub fn paste(&mut self, text: &str) -> Option<Effect> {
        let edit = self.edit.as_mut()?;
        edit.input.insert(text);
        edit.error = None;
        let field = edit.field;
        self.typed(field)
    }

    fn confirm_key(&mut self, key: &str) -> Option<Effect> {
        match key {
            "y" => {
                self.confirm_exit = false;
                let effect = self.save();
                if effect == Some(Effect::Save) {
                    self.close_after_save = true;
                }
                effect
            }
            "n" => {
                self.confirm_exit = false;
                Some(Effect::Close)
            }
            "esc" => {
                self.confirm_exit = false;
                None
            }
            _ => None,
        }
    }

    fn set_section(&mut self, section: Section) {
        self.section = section;
        self.focus = 0;
        self.scroll = 0;
    }

    /// Moves the focus; a section without fields scrolls instead (the view
    /// clamps the scroll to its content).
    fn move_focus(&mut self, delta: isize) {
        let n = self.section.fields().len();
        if n == 0 {
            self.scroll = self.scroll.saturating_add_signed(delta);
            return;
        }
        self.focus = (self.focus as isize + delta).clamp(0, n as isize - 1) as usize;
    }

    fn activate(&mut self) -> Option<Effect> {
        let field = self.field()?;
        if field.is_toggle() {
            return self.toggle();
        }
        if field.is_text() {
            self.open_edit();
        }
        None
    }

    fn open_edit(&mut self) {
        let Some(field) = self.field() else { return };
        let mut input = TextInput::new("", "");
        match field {
            Field::Url => input.set_value(&self.draft.proxy.url),
            Field::ClaudePrefix => input.set_value(&self.draft.proxy.claude.model_prefix),
            Field::CodexPrefix => input.set_value(&self.draft.proxy.codex.model_prefix),
            // A key edit starts empty: the saved key is never shown.
            _ => {}
        }
        input.focus();
        self.edit = Some(FieldEdit {
            field,
            input,
            error: None,
        });
    }

    fn edit_key(&mut self, key: &str, ev: &KeyEvent) -> Option<Effect> {
        let edit = self.edit.as_mut()?;
        match key {
            "esc" => {
                let field = edit.field;
                self.edit = None;
                // The dot follows the typed URL; put it back on the draft.
                return (field == Field::Url).then_some(Effect::Probe { debounce: false });
            }
            "enter" => return self.commit_edit(),
            _ => {}
        }
        let before = edit.input.value();
        edit.input.handle_key(ev);
        if edit.input.value() == before {
            return None;
        }
        edit.error = None;
        let field = edit.field;
        self.typed(field)
    }

    /// After the text of a field edit changed: the URL is tried again once
    /// typing pauses.
    fn typed(&mut self, field: Field) -> Option<Effect> {
        (field == Field::Url).then_some(Effect::Probe { debounce: true })
    }

    fn commit_edit(&mut self) -> Option<Effect> {
        let edit = self.edit.as_mut()?;
        let text = edit.input.value();
        match edit.field {
            Field::Key => {
                if text.trim().is_empty() {
                    self.edit = None;
                    return None;
                }
                if text
                    .trim()
                    .chars()
                    .any(|c| c.is_whitespace() || c.is_control())
                {
                    edit.error = Some("the key has a space; paste one key on one line".into());
                    return None;
                }
                self.pending_key = Some(Secret::new(&text));
                self.edit = None;
                if env_key_wins(&self.seed.cfg) {
                    self.info("RIVAL_PROXY_KEY is set and wins over the key file");
                }
                self.validate();
                return Some(Effect::Probe { debounce: false });
            }
            Field::Url => self.draft.proxy.url = text.trim().to_string(),
            Field::ClaudePrefix => {
                self.draft.proxy.claude.model_prefix = config::normalize_model_prefix(&text);
            }
            Field::CodexPrefix => {
                self.draft.proxy.codex.model_prefix = config::normalize_model_prefix(&text);
            }
            _ => {}
        }
        let field = edit.field;
        self.edit = None;
        self.validate();
        (field == Field::Url).then_some(Effect::Probe { debounce: false })
    }

    fn toggle(&mut self) -> Option<Effect> {
        match self.field()? {
            Field::ClaudeRoute => {
                self.draft.proxy.claude.enabled = !self.draft.proxy.claude.enabled;
            }
            Field::CodexRoute => self.draft.proxy.codex.enabled = !self.draft.proxy.codex.enabled,
            Field::AutoFix => {
                self.draft.auto_fix_critical_high = !self.draft.auto_fix_critical_high;
            }
            Field::Model(i) => self.toggle_plan(i),
            _ => return None,
        }
        self.validate();
        None
    }

    fn toggle_plan(&mut self, i: usize) {
        let Some(m) = MODELS.get(i) else { return };
        if !m.plan {
            self.info(&format!("{} is not a plan reviewer", m.name));
            return;
        }
        let mut chosen = plan_set(&self.draft);
        if chosen.contains(&m.name) {
            if chosen.len() == 1 {
                self.error_notice("the plan review needs at least one model");
                return;
            }
            chosen.retain(|n| *n != m.name);
        } else {
            chosen.push(m.name);
        }
        self.draft.plan.models = MODELS
            .iter()
            .filter(|m| chosen.contains(&m.name))
            .map(|m| m.name.to_string())
            .collect();
    }

    /// `←→` on a picker.
    fn step(&mut self, delta: isize) -> Option<Effect> {
        let field = self.field()?;
        match field {
            Field::ClaudePrefix | Field::CodexPrefix => {
                let choices = self.prefix_choices(field.provider()?);
                let current = self.prefix(field.provider()?).to_string();
                let next = cycle(&choices, &current, delta);
                match field {
                    Field::ClaudePrefix => self.draft.proxy.claude.model_prefix = next,
                    _ => self.draft.proxy.codex.model_prefix = next,
                }
            }
            Field::Reviewer => {
                let names: Vec<String> = REVIEWERS.iter().map(|(n, _)| n.to_string()).collect();
                self.draft.security.reviewer = cycle(&names, self.reviewer(), delta);
            }
            Field::Model(i) => {
                let m = MODELS.get(i)?;
                if m.ladder.len() < 2 {
                    self.info(&format!("{} runs at {} only", m.display, m.ladder[0]));
                    return None;
                }
                let ladder: Vec<String> = m.ladder.iter().map(|s| s.to_string()).collect();
                let next = cycle(&ladder, &self.effort(m), delta);
                self.draft.efforts.insert(m.effort_label.to_string(), next);
            }
            Field::ClaudeRoute | Field::CodexRoute | Field::AutoFix => return self.toggle(),
            _ => return None,
        }
        self.validate();
        None
    }

    fn import_base_url(&mut self) -> Option<Effect> {
        if self.field() != Some(Field::Url) {
            return None;
        }
        if self.seed.base_url_env.is_empty() {
            self.info("ANTHROPIC_BASE_URL is not set");
            return None;
        }
        self.draft.proxy.url = self.seed.base_url_env.clone();
        self.validate();
        self.info("imported ANTHROPIC_BASE_URL");
        Some(Effect::Probe { debounce: false })
    }

    fn save(&mut self) -> Option<Effect> {
        if self.saving {
            return None;
        }
        self.validate();
        if let Some(why) = self.blocked() {
            self.error_notice(&format!("save blocked: {why}"));
            return None;
        }
        if !self.dirty() {
            self.info("nothing to save");
            return None;
        }
        self.saving = true;
        Some(Effect::Save)
    }

    /// Why `s` cannot save now, if it cannot.
    pub fn blocked(&self) -> Option<String> {
        if let Some((field, e)) = self.errors.first() {
            return Some(format!("{} — {e}", field.label()));
        }
        self.general_error.clone()
    }

    fn revert(&mut self) -> Option<Effect> {
        if !self.dirty() {
            self.info("nothing to undo");
            return None;
        }
        self.draft = self.seed.saved.clone();
        self.pending_key = None;
        self.validate();
        self.info("draft dropped");
        Some(Effect::Probe { debounce: false })
    }

    fn info(&mut self, text: &str) {
        self.notice = Some(Notice {
            text: text.to_string(),
            kind: NoticeKind::Info,
        });
    }

    fn error_notice(&mut self, text: &str) {
        self.notice = Some(Notice {
            text: text.to_string(),
            kind: NoticeKind::Error,
        });
    }

    // --- the draft ----------------------------------------------------------

    /// The draft's prefix for `provider`.
    pub fn prefix(&self, provider: ProxyProvider) -> &str {
        match provider {
            ProxyProvider::Claude => &self.draft.proxy.claude.model_prefix,
            ProxyProvider::Codex => &self.draft.proxy.codex.model_prefix,
        }
    }

    pub fn route_enabled(&self, provider: ProxyProvider) -> bool {
        match provider {
            ProxyProvider::Claude => self.draft.proxy.claude.enabled,
            ProxyProvider::Codex => self.draft.proxy.codex.enabled,
        }
    }

    /// Whether `provider`'s runs go through the proxy with the draft.
    pub fn proxied(&self, provider: Option<ProxyProvider>) -> bool {
        provider.is_some_and(|p| self.route_enabled(p)) && !self.seed.proxy_off_env
    }

    /// The model id a run of `m` sends with the draft.
    pub fn wire(&self, m: &ModelInfo) -> String {
        match m.provider {
            Some(p) if self.proxied(Some(p)) => proxy::wire_id(self.prefix(p), m.model),
            _ => m.model.to_string(),
        }
    }

    /// The prefix picker's values: the prefixes whose models include one of
    /// `provider`'s, from the last proxy answer, then `none` (""). The
    /// draft's own value is kept when the proxy does not list it.
    ///
    /// Once the proxy answered, a prefix is offered only when its ids
    /// include one of `provider`'s rival models, and `none` only when the
    /// proxy serves one of them bare (it serves no bare Claude id). Before
    /// that (or offline) every value is open: the draft's and `none`.
    pub fn prefix_choices(&self, provider: ProxyProvider) -> Vec<String> {
        let ids = &self.probe.ids;
        let models: Vec<&str> = MODELS
            .iter()
            .filter(|m| m.provider == Some(provider))
            .map(|m| m.model)
            .collect();
        // Sorted and without repeats: the picker order is stable.
        let mut out: Vec<String> = ids
            .iter()
            .filter(|id| models.contains(&proxy::id_model(id)))
            .map(|id| proxy::id_prefix(id).to_string())
            .filter(|p| !p.is_empty())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        let current = self.prefix(provider);
        if !current.is_empty() && !out.iter().any(|p| p == current) {
            out.push(current.to_string());
        }
        let bare_served = models.iter().any(|m| ids.iter().any(|id| id == m));
        if ids.is_empty() || bare_served || current.is_empty() {
            out.push(String::new());
        }
        out
    }

    /// Whether the proxy, once it answered, serves `provider`'s models with
    /// the draft's prefix. `Some` is the warning for the first one it does
    /// not serve; it never blocks a save.
    pub fn prefix_warning(&self, provider: ProxyProvider) -> Option<String> {
        if self.probe.ids.is_empty() {
            return None;
        }
        let prefix = self.prefix(provider);
        MODELS
            .iter()
            .filter(|m| m.provider == Some(provider))
            .map(|m| (m.model, proxy::wire_id(prefix, m.model)))
            .find(|(_, wire)| !self.probe.ids.iter().any(|id| id == wire))
            .map(|(model, wire)| format!("the proxy does not serve {model} as {wire}"))
    }

    /// The draft's effort for `m`.
    pub fn effort(&self, m: &ModelInfo) -> String {
        self.draft
            .efforts
            .get(m.effort_label)
            .cloned()
            .unwrap_or_else(|| config::builtin_effort(m.effort_label).to_string())
    }

    /// The draft's security reviewer.
    pub fn reviewer(&self) -> &str {
        effective_reviewer(&self.draft)
    }

    /// Whether `m` is a plan default in the draft.
    pub fn plan_default(&self, m: &ModelInfo) -> bool {
        m.plan && plan_set(&self.draft).contains(&m.name)
    }

    /// The draft's difference from the saved config as a `config set --json`
    /// patch with dotted keys.
    pub fn patch(&self) -> Json {
        Json::Object(self.patch_map())
    }

    fn patch_map(&self) -> Map<String, Json> {
        patch_between(&self.seed.saved, &self.draft)
    }

    /// The patch as writer edits.
    pub fn edits(&self) -> Result<Vec<Edit>, String> {
        edits_of(&self.patch())
    }

    /// The draft as a config: the saved file text with the patch, through
    /// the writer's text and the config parser, with a typed key in place
    /// of the key file.
    pub fn draft_config(&self) -> Result<Config, String> {
        self.config_with(&self.draft)
    }

    fn config_with(&self, draft: &UserConfig) -> Result<Config, String> {
        let edits = edits_of(&Json::Object(patch_between(&self.seed.saved, draft)))?;
        let (_, user) = write::render(self.seed.text.as_deref(), &edits, &self.seed.path)
            .map_err(|e| e.to_string())?;
        let cfg = self.seed.cfg.clone().with_user_config(Some(user));
        Ok(match &self.pending_key {
            Some(key) => cfg.with_draft_proxy_key(key.expose()),
            None => cfg,
        })
    }

    /// Checks every field of the draft, then the draft as a whole.
    pub fn validate(&mut self) {
        let mut errors = Vec::new();
        let d = &self.draft;
        let url = d.proxy.url.trim();
        if !url.is_empty() {
            if let Err(why) = config::normalize_proxy_url(url) {
                errors.push((Field::Url, why));
            }
        } else if (d.proxy.claude.enabled || d.proxy.codex.enabled) && self.seed.url_env.is_empty()
        {
            errors.push((
                Field::Url,
                "a route goes through the proxy; set its URL or turn both routes off".to_string(),
            ));
        }
        for (field, prefix) in [
            (Field::ClaudePrefix, &d.proxy.claude.model_prefix),
            (Field::CodexPrefix, &d.proxy.codex.model_prefix),
        ] {
            if prefix.chars().any(|c| c.is_whitespace() || c == '/') {
                errors.push((field, "a prefix has no spaces and no /".to_string()));
            }
        }
        self.general_error = None;
        if errors.is_empty()
            && let Err(e) = self.draft_config()
        {
            self.general_error = Some(e);
        }
        self.errors = errors;
    }

    // --- jobs ---------------------------------------------------------------

    /// The config a probe asks with: the draft, with the URL being typed.
    /// `None` (and the dot idle) when there is no URL to try.
    pub fn probe_config(&self) -> Option<Config> {
        let mut draft = self.draft.clone();
        if let Some(edit) = &self.edit
            && edit.field == Field::Url
        {
            draft.proxy.url = edit.input.value().trim().to_string();
        }
        let url = draft.proxy.url.trim();
        let from_env = !self.seed.url_env.is_empty();
        if !from_env && (url.is_empty() || config::normalize_proxy_url(url).is_err()) {
            return None;
        }
        // Prefix errors must not keep the dot from the URL.
        draft.proxy.claude.model_prefix = self.seed.saved.proxy.claude.model_prefix.clone();
        draft.proxy.codex.model_prefix = self.seed.saved.proxy.codex.model_prefix.clone();
        self.config_with(&draft).ok()
    }

    /// A new probe is due: older answers and pending timers are void.
    /// Returns its number.
    pub fn bump_probe(&mut self) -> u64 {
        self.seed.serial += 1;
        self.probe.seq = self.seed.serial;
        self.probe.seq
    }

    /// The request of the newest probe, or `None` with the dot idle.
    pub fn probe_request(&mut self) -> Option<ProbeRequest> {
        match self.probe_config() {
            Some(cfg) => {
                self.probe.state = ProbeState::Waiting;
                Some(ProbeRequest {
                    seq: self.probe.seq,
                    cfg,
                })
            }
            None => {
                self.probe.state = ProbeState::Idle;
                None
            }
        }
    }

    /// Takes a probe's answer, if it is the newest.
    pub fn probe_done(&mut self, seq: u64, result: Result<Vec<String>, String>) {
        if seq != self.probe.seq {
            return;
        }
        match result {
            Ok(ids) => {
                self.probe.state = ProbeState::Online(ids.len());
                self.probe.ids = ids;
            }
            Err(e) => self.probe.state = ProbeState::Offline(e),
        }
    }

    /// Starts a check on the draft. `Err` is the notice.
    pub fn start_check(&mut self, all: bool) -> Result<CheckRequest, String> {
        if self.check.running {
            return Err("a check is running — x cancels it".to_string());
        }
        let cfg = self
            .draft_config()
            .map_err(|e| format!("check blocked: {e}"))?;
        let names: Vec<String> = if all {
            vec!["all".to_string()]
        } else {
            Vec::new()
        };
        let targets = check::targets(&cfg, &names)?;
        let (ctx, cancel) = Context::background().with_cancel();
        self.seed.serial += 1;
        self.check.run = self.seed.serial;
        self.check.running = true;
        self.check.cancelling = false;
        self.check.cancel = Some(cancel);
        self.check.proxy = None;
        self.check.slots = targets
            .iter()
            .map(|t| {
                let info = MODELS.iter().find(|m| m.name == t.name);
                let provider = t.provider();
                let proxied = self.proxied(provider);
                CheckSlot {
                    name: t.name.to_string(),
                    route: if proxied { "proxy" } else { "direct" }.to_string(),
                    wire: match (info, provider) {
                        (Some(m), _) => self.wire(m),
                        (None, Some(p)) if proxied => proxy::wire_id(self.prefix(p), t.model),
                        _ => t.model.to_string(),
                    },
                    row: None,
                }
            })
            .collect();
        Ok(CheckRequest {
            run: self.check.run,
            cfg,
            targets,
            ctx,
        })
    }

    /// One finished row of the newest run.
    pub fn check_row(&mut self, run: u64, row: CheckRow) {
        if run != self.check.run {
            return;
        }
        if let Some(slot) = self.check.slots.iter_mut().find(|s| s.name == row.name) {
            slot.route = row.route.clone();
            slot.wire = row.wire_model.clone();
            slot.row = Some(row);
        }
    }

    /// The end of the newest run, at `now`.
    pub fn check_done(&mut self, run: u64, report: Report, now: DateTime<FixedOffset>) {
        if run != self.check.run {
            return;
        }
        for row in report.rows {
            self.check_row(run, row);
        }
        self.check.proxy = Some(report.proxy);
        self.check.running = false;
        self.check.cancelling = false;
        self.check.cancel = None;
        self.check.finished = Some(now);
    }

    /// Cancels a running check (closing the window, quitting).
    pub fn cancel_check(&mut self) {
        self.check.cancel();
    }

    /// The save job for the draft.
    pub fn save_request(&self) -> Result<SaveRequest, String> {
        Ok(SaveRequest {
            path: self.seed.path.clone(),
            edits: self.edits()?,
            key: self.pending_key.clone(),
            key_path: self.seed.key_path.clone(),
        })
    }

    /// The save landed: the draft is the saved config now. Returns whether
    /// the window should close.
    pub fn saved(&mut self, saved: &Saved) -> bool {
        self.saving = false;
        self.seed.saved(saved);
        self.draft = self.seed.saved.clone();
        self.pending_key = None;
        self.validate();
        self.notice = Some(Notice {
            text: format!("saved {}", self.seed.path_shown),
            kind: NoticeKind::Ok,
        });
        std::mem::take(&mut self.close_after_save)
    }

    /// A check that could not start.
    pub fn check_refused(&mut self, why: &str) {
        self.error_notice(why);
    }

    pub fn save_failed(&mut self, error: &str) {
        self.saving = false;
        self.close_after_save = false;
        self.error_notice(&format!("save failed: {error}"));
    }

    /// The saved key's status, or the typed key's.
    pub fn key_view(&self) -> KeyStatus {
        match (&self.pending_key, &self.seed.key) {
            (Some(_), KeyStatus::Set { env: true, .. }) => self.seed.key.clone(),
            (Some(key), _) => KeyStatus::Set {
                tail: key.tail(),
                env: false,
                mode: None,
            },
            (None, status) => status.clone(),
        }
    }
}

/// The difference of `d` from `s` as a patch with dotted keys. Efforts,
/// plan models and the reviewer compare by their effective values, so a
/// default chosen again is no change.
fn patch_between(s: &UserConfig, d: &UserConfig) -> Map<String, Json> {
    let mut p = Map::new();
    let url = d.proxy.url.trim();
    if url != s.proxy.url {
        p.insert(
            "proxy.url".into(),
            if url.is_empty() {
                Json::Null
            } else {
                json!(url)
            },
        );
    }
    for (name, sr, dr) in [
        ("claude", &s.proxy.claude, &d.proxy.claude),
        ("codex", &s.proxy.codex, &d.proxy.codex),
    ] {
        if sr.enabled != dr.enabled {
            p.insert(format!("proxy.{name}.enabled"), json!(dr.enabled));
        }
        if sr.model_prefix != dr.model_prefix {
            p.insert(format!("proxy.{name}.model_prefix"), json!(dr.model_prefix));
        }
    }
    for m in &MODELS {
        let effort = |u: &UserConfig| {
            u.efforts
                .get(m.effort_label)
                .cloned()
                .unwrap_or_else(|| config::builtin_effort(m.effort_label).to_string())
        };
        if effort(s) != effort(d) {
            p.insert(format!("efforts.{}", m.effort_label), json!(effort(d)));
        }
    }
    let (mut sp, mut dp) = (plan_set(s), plan_set(d));
    sp.sort_unstable();
    dp.sort_unstable();
    if sp != dp {
        p.insert("plan.models".into(), json!(d.plan.models));
    }
    if effective_reviewer(s) != effective_reviewer(d) {
        p.insert("security.reviewer".into(), json!(effective_reviewer(d)));
    }
    if s.auto_fix_critical_high != d.auto_fix_critical_high {
        p.insert(
            "auto_fix_critical_high".into(),
            json!(d.auto_fix_critical_high),
        );
    }
    p
}

fn edits_of(patch: &Json) -> Result<Vec<Edit>, String> {
    write::edits_from_json(patch).map_err(|e| e.to_string())
}

/// The configured reviewer, `k3` when unset.
fn effective_reviewer(u: &UserConfig) -> &str {
    match u.security.reviewer.trim() {
        "" => SECURITY_REVIEWER_K3,
        r => r,
    }
}

/// The plan models of `u` as row names: `opus` is `claude`; none set means
/// `codex`.
fn plan_set(u: &UserConfig) -> Vec<&'static str> {
    let names: Vec<&str> = if u.plan.models.is_empty() {
        vec![CODEX_LABEL]
    } else {
        u.plan
            .models
            .iter()
            .map(|n| {
                if n == config::OPUS_ALIAS {
                    CLAUDE_LABEL
                } else {
                    n.as_str()
                }
            })
            .collect()
    };
    MODELS
        .iter()
        .filter(|m| m.plan && names.contains(&m.name))
        .map(|m| m.name)
        .collect()
}

/// The value `delta` steps from `current` in `values`, wrapping. A value
/// not in the list steps to the first (or the last).
fn cycle(values: &[String], current: &str, delta: isize) -> String {
    if values.is_empty() {
        return current.to_string();
    }
    let n = values.len() as isize;
    let next = match values.iter().position(|v| v == current) {
        Some(i) => (i as isize + delta).rem_euclid(n),
        None if delta < 0 => n - 1,
        None => 0,
    };
    values[next as usize].clone()
}
