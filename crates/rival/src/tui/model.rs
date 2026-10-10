//! The dashboard model: state, message routing and the frame.
//!
//! The model never touches the terminal, the session files, the processes
//! or the clock on its own. The runtime (Task 4.5) feeds it [`Msg`]s from
//! the watcher, the keyboard, its timers and its job workers, carries out the
//! returned [`Cmd`]s and draws it with [`Model::draw`]. Blocking work (log
//! reads, prompt loads, stops) leaves as [`Cmd::Job`] and comes back as a
//! result message. Tests drive it the same way through a ratatui
//! `TestBackend`.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, FixedOffset, Local};
use crossterm::event::KeyEvent;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Padding, Widget};

use rival_core::session::Session;
use rival_core::sessionview::{self, Bucket, LoadProgress, SessionEvent};

use crate::check::{CheckRow, Report};

use super::config_check::{ConfigSeed, ProbeResult, SaveResult};
use super::config_form::{ConfigForm, Effect, PROBE_DEBOUNCE};
use super::config_view::{self, ViewCtx};
use super::detail_view::{DetailPane, DetailTab, KillConfirm};
use super::jobs::{Job, OpenLogRequest, PromptsResult};
use super::keys::{
    ARROW_DOWN, ARROW_UP, FORCE_QUIT, KeyMap, Mode, OPEN_CONFIG, help_lines, key_name,
};
use super::kill::{
    AliveFn, StopRequest, StopResult, UNVERIFIED_NOTICE, has_unverified, live_targets,
    recheck_targets, same_process,
};
use super::layout::{Layout, MIN_HEIGHT, MIN_WIDTH, compute_layout};
use super::logview::{LogPane, LogResult};
use super::preview::PreviewPane;
use super::result_view::ResultResponse;
use super::session_list::{ListPane, Zone, is_live, item_status};
use super::styles::{HeaderStats, STYLES, Styles, gradient_bar, render_header};
use super::text::{fit_line, line_width, pad_line, truncate};

/// One or more sessions shown as one row: a solo run, or every member of a
/// group run.
#[derive(Debug, Clone, PartialEq)]
pub struct DisplayItem {
    pub sessions: Vec<Arc<Session>>,
}

impl DisplayItem {
    /// The first session, used for shared metadata.
    pub fn primary(&self) -> Option<&Arc<Session>> {
        self.sessions.first()
    }

    /// True for a logical grouped run, including a degraded run where only
    /// one requested model passed preflight.
    pub fn is_group(&self) -> bool {
        self.sessions.len() > 1
            || self
                .sessions
                .first()
                .is_some_and(|s| !s.group_id.is_empty())
    }
}

impl From<Bucket> for DisplayItem {
    fn from(b: Bucket) -> Self {
        DisplayItem {
            sessions: b.sessions,
        }
    }
}

/// Merges sessions sharing a group id into display items. The bucketing
/// itself lives in `sessionview`.
pub fn group_sessions(sessions: &[Arc<Session>]) -> Vec<DisplayItem> {
    sessionview::group(sessions)
        .into_iter()
        .map(DisplayItem::from)
        .collect()
}

/// Identifies a display item across refreshes. A group is keyed by its
/// group id, a solo session by its own id; both are stable for the run's
/// lifetime.
pub fn item_key(item: &DisplayItem) -> String {
    match item.primary() {
        None => String::new(),
        Some(s) if !s.group_id.is_empty() => format!("group:{}", s.group_id),
        Some(s) => format!("solo:{}", s.id),
    }
}

/// Whether a row is still running or waiting.
pub fn item_live(item: &DisplayItem) -> bool {
    is_live(item_status(item))
}

/// What the views need besides their own state: the clock, the local zone
/// and the theme.
#[derive(Debug, Clone, Copy)]
pub struct Ctx<'a> {
    pub now: DateTime<FixedOffset>,
    pub zone: Zone,
    pub styles: &'a Styles,
}

/// Everything the model reacts to.
#[derive(Debug, Clone, PartialEq)]
pub enum Msg {
    Key(KeyEvent),
    /// Bracketed paste: lands in whichever text input has focus.
    Paste(String),
    Resize {
        width: u16,
        height: u16,
    },
    /// A watcher snapshot.
    Sessions(SessionEvent),
    /// The initial scan's progress, before the first snapshot.
    Progress(LoadProgress),
    /// The 1s refresh tick ([`TICK_INTERVAL`]).
    Tick,
    /// A spinner frame tick ([`SPIN_INTERVAL`]).
    SpinTick,
    /// The watcher failed to start; no snapshot is coming.
    Error(String),
    /// A [`Job::Log`] finished.
    Log(LogResult),
    /// A [`Job::Result`] finished.
    Result(ResultResponse),
    /// A [`Job::Prompts`] finished.
    Prompts(PromptsResult),
    /// A [`Job::Stop`] finished.
    Stopped(StopResult),
    /// The runtime could not queue a stop or a log open; the text says so.
    Rejected(String),
    /// The config window's typing pause is over: probe `n` is due, unless
    /// a newer one replaced it.
    ConfigProbe(u64),
    /// A [`Job::Probe`] finished.
    ProxyModels(ProbeResult),
    /// One model of check `run` finished.
    CheckRow {
        run: u64,
        row: Box<CheckRow>,
    },
    /// Check `run` finished.
    CheckDone {
        run: u64,
        report: Box<Report>,
    },
    /// A [`Job::Save`] finished.
    ConfigSaved(Box<SaveResult>),
}

/// What the runtime must do after an update.
#[derive(Debug, Clone, PartialEq)]
pub enum Cmd {
    /// Stop the watcher and leave the terminal.
    Quit,
    /// Send [`Msg::Tick`] after [`TICK_INTERVAL`].
    Tick,
    /// Send [`Msg::SpinTick`] after [`SPIN_INTERVAL`].
    Spin,
    /// Send the message after the delay (the config window's typing pause).
    After(Duration, Msg),
    /// Run blocking work off the UI thread with `JobEnv::run` and send back
    /// the message it returns. Results may arrive late or out of order; the
    /// model drops the ones the screen no longer needs.
    Job(Job),
}

/// The refresh tick for live timers and log tails.
pub const TICK_INTERVAL: Duration = Duration::from_secs(1);
/// The spinner frame rate (bubbles `MiniDot`: 12 fps).
pub const SPIN_INTERVAL: Duration = Duration::from_millis(1000 / 12);
/// Bubbles' `MiniDot` spinner.
const SPIN_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Caps the loader bar so it reads as a bar, not a rule.
const LOADER_BAR_MAX: usize = 48;

/// The dashboard state.
#[derive(Debug, Clone)]
pub struct Model {
    pub(crate) lay: Layout,
    /// The help bar's height in rows. It depends on the mode, "?" and the
    /// width, so `measure_help` recomputes it when one of those changes.
    pub(crate) help_h: usize,
    /// Turns true on the first snapshot. Until then the body is the loader
    /// and the counts are "…", never a misleading 0.
    pub(crate) loaded: bool,
    pub(crate) load: LoadProgress,
    pub(crate) err_text: String,
    pub(crate) quitting: bool,
    pub(crate) keys: KeyMap,
    /// "?" expands the help bar.
    pub(crate) show_all_help: bool,
    pub(crate) mode: Mode,
    pub(crate) list: ListPane,
    pub(crate) detail: DetailPane,
    pub(crate) preview: PreviewPane,
    /// Authorizes opening the stop confirm: whether a PID is still the
    /// process that started at the recorded time. The stop job checks again
    /// right before it signals. Tests replace it with fakes.
    pub(crate) alive: AliveFn,
    /// Jobs queued during the current update, returned with its commands.
    jobs: Vec<Cmd>,
    pub(crate) spin_frame: usize,
    /// Whether a spinner tick is in flight, so a new event never starts a
    /// second, faster chain.
    pub(crate) spinning: bool,
    /// The same guard for the 1s refresh tick.
    pub(crate) ticking: bool,
    pub(crate) version: String,
    pub(crate) styles: Styles,
    /// The wall clock for day sections and elapsed times; tests pin it.
    pub(crate) clock: fn() -> DateTime<FixedOffset>,
    /// The open config window.
    pub(crate) config: Option<ConfigForm>,
    /// What the config window opens with; a save updates it.
    pub(crate) config_seed: Option<ConfigSeed>,
    /// `rival config`: the window is the whole TUI, and leaving it quits.
    pub(crate) config_only: bool,
}

fn local_now() -> DateTime<FixedOffset> {
    Local::now().fixed_offset()
}

impl Model {
    /// A model for rival `version`. Call [`Model::init`] once to get the
    /// first commands.
    pub fn new(version: impl Into<String>) -> Model {
        let mut m = Model {
            lay: Layout::default(),
            help_h: 1,
            loaded: false,
            load: LoadProgress { done: 0, total: 0 },
            err_text: String::new(),
            quitting: false,
            keys: KeyMap::default(),
            show_all_help: false,
            mode: Mode::List,
            list: ListPane::default(),
            detail: DetailPane::default(),
            preview: PreviewPane::default(),
            alive: same_process,
            jobs: Vec::new(),
            spin_frame: 0,
            // Init starts the spinner chain for the loader.
            spinning: true,
            ticking: false,
            version: version.into(),
            styles: STYLES,
            clock: local_now,
            config: None,
            config_seed: None,
            config_only: false,
        };
        m.measure_help();
        m
    }

    /// The commands to run at startup: the loader's spinner.
    pub fn init(&self) -> Vec<Cmd> {
        vec![Cmd::Spin]
    }

    /// What `c` opens the config window with.
    pub fn set_config_seed(&mut self, seed: ConfigSeed) {
        self.config_seed = Some(seed);
    }

    /// `rival config`: opens the window with nothing behind it. Returns the
    /// first commands (the proxy probe).
    pub fn open_config_only(&mut self, seed: ConfigSeed) -> Vec<Cmd> {
        self.config_only = true;
        // No session watch runs, so the loader never shows.
        self.loaded = true;
        self.config_seed = Some(seed);
        self.open_config();
        std::mem::take(&mut self.jobs)
    }

    /// Cancels work that would outlive the screen: a running check.
    pub fn cancel_work(&mut self) {
        if let Some(form) = self.config.as_mut() {
            form.cancel_check();
        }
    }

    /// Whether the config window shows.
    pub fn in_config(&self) -> bool {
        matches!(self.mode, Mode::Config | Mode::ConfigEdit) && self.config.is_some()
    }

    /// Whether the user asked to quit.
    #[cfg(test)]
    pub fn quitting(&self) -> bool {
        self.quitting
    }

    fn now(&self) -> DateTime<FixedOffset> {
        (self.clock)()
    }

    /// The clock, zone and theme the views draw with.
    pub(crate) fn ctx(&self) -> Ctx<'_> {
        Ctx {
            now: self.now(),
            zone: self.list.zone,
            styles: &self.styles,
        }
    }

    /// Handles one message and returns what the runtime must do next.
    pub fn update(&mut self, msg: Msg) -> Vec<Cmd> {
        let prev_help = self.help_state();
        let spin = matches!(msg, Msg::SpinTick);
        // A tick and a snapshot check the shown logs for growth; other
        // messages only read a log the screen does not hold yet.
        let refresh = matches!(msg, Msg::Tick | Msg::Sessions(_));
        let mut cmds = self.route(msg);
        // The help bar's height depends on the mode, on "?" and on whether a
        // Result tab lists findings. When one changes the detail viewport
        // must be resized, or it keeps a height the view then clips.
        if self.help_state() != prev_help {
            self.measure_help();
            self.resize_detail();
        }
        // A spinner frame changes nothing the list or the preview show.
        if !spin {
            self.reconcile(refresh);
        }
        if self.quitting {
            self.jobs.clear();
        } else {
            cmds.append(&mut self.jobs);
        }
        cmds
    }

    /// Runs once after every update: scrolls the list so the cursor stays in
    /// view and asks for the preview's log when the selection, the size or a
    /// refresh needs it.
    fn reconcile(&mut self, refresh: bool) {
        let rows = self.list_rows();
        self.list.clamp_offset(rows);
        self.sync_preview(refresh);
    }

    /// The detail screen hides the preview, so nothing is
    /// read there.
    fn sync_preview(&mut self, refresh: bool) {
        if !self.preview_shown() {
            return;
        }
        let (w, h) = (self.preview_inner_width(), self.list_inner_height());
        let ctx = Ctx {
            now: self.now(),
            zone: self.list.zone,
            styles: &self.styles,
        };
        if let Some(req) = self
            .preview
            .request(self.list.selected(), w, h, &ctx, refresh)
        {
            self.jobs.push(Cmd::Job(Job::Log(req)));
        }
    }

    fn preview_shown(&self) -> bool {
        self.lay.show_preview && !self.lay.too_small && !self.in_detail() && !self.in_config()
    }

    /// Takes a finished log read. Each pane drops a read it no longer
    /// needs: another member, another width, or an older result.
    fn accept_log(&mut self, res: LogResult) {
        let ctx = Ctx {
            now: self.now(),
            zone: self.list.zone,
            styles: &self.styles,
        };
        match res.pane {
            LogPane::Detail if self.in_detail() => {
                self.detail.accept_log(res, self.list.selected(), &ctx);
            }
            LogPane::Detail => {
                self.detail.log.accept(res, None);
            }
            LogPane::Preview if self.preview_shown() => {
                let (w, h) = (self.preview_inner_width(), self.list_inner_height());
                self.preview.accept(res, self.list.selected(), w, h, &ctx);
            }
            LogPane::Preview => {
                self.preview.tail.accept(res, None);
            }
        }
    }

    /// The end of a confirmed stop. The stop wrote the targets as
    /// failed; the rows on screen copy what was stored, the header counts
    /// them now, and the open detail redraws.
    fn apply_stop(&mut self, res: StopResult) {
        for item in &mut self.list.items {
            for s in &mut item.sessions {
                if let Some((_, update)) = res.updates.iter().find(|(id, _)| *id == s.id) {
                    update.apply(Arc::make_mut(s));
                }
            }
        }
        self.list.count_statuses();
        self.sync_detail(false);
    }

    fn route(&mut self, msg: Msg) -> Vec<Cmd> {
        match msg {
            Msg::Key(ev) => {
                let Some(key) = key_name(&ev) else {
                    return Vec::new();
                };
                if FORCE_QUIT.matches(&key) {
                    return self.quit();
                }
                match self.mode {
                    Mode::Filter => self.filter_key(&key, &ev),
                    Mode::Detail => self.detail_key(&key),
                    Mode::Search => self.search_key(&key, &ev),
                    Mode::Confirm => self.confirm_key(&key),
                    Mode::List => self.list_key(&key),
                    Mode::Config | Mode::ConfigEdit => self.config_key(&key, &ev),
                }
            }
            Msg::Paste(text) => {
                // A paste arrives here, not as keys, so it must refilter too.
                match self.mode {
                    Mode::Filter => {
                        let before = self.list.filter.value();
                        self.list.filter.insert(&text);
                        if self.list.filter.value() != before {
                            let now = self.now();
                            self.list.refilter(now);
                        }
                    }
                    Mode::Search => self.detail.search.insert(&text),
                    Mode::ConfigEdit => {
                        let effect = self.config.as_mut().and_then(|f| f.paste(&text));
                        self.config_effect(effect);
                    }
                    _ => {}
                }
                Vec::new()
            }
            Msg::Resize { width, height } => {
                self.lay = compute_layout(usize::from(width), usize::from(height));
                self.measure_help();
                self.sync_detail(false);
                Vec::new()
            }
            Msg::Sessions(ev) => {
                self.loaded = true;
                self.apply_sessions(&ev.sessions);
                self.ensure_live_timers()
            }
            Msg::Tick => {
                // Re-read the open run while it can still grow. Keep ticking
                // while anything runs.
                if self.in_detail() && self.list.selected().is_some_and(item_live) {
                    self.sync_detail(false);
                }
                if self.list.any_live {
                    return vec![Cmd::Tick];
                }
                self.ticking = false;
                Vec::new()
            }
            Msg::Progress(p) => {
                if !self.loaded {
                    self.load = p;
                }
                Vec::new()
            }
            Msg::SpinTick => {
                // Dropping the tick ends the chain; the next snapshot with a
                // running row restarts it. The loader keeps it alive.
                if self.loaded && !self.list.any_live && !self.config_busy() {
                    self.spinning = false;
                    return Vec::new();
                }
                self.spin_frame = (self.spin_frame + 1) % SPIN_FRAMES.len();
                vec![Cmd::Spin]
            }
            Msg::Error(text) => {
                self.err_text = text;
                // No snapshot is coming; let the loader's spinner chain end.
                self.loaded = true;
                Vec::new()
            }
            Msg::Log(res) => {
                self.accept_log(res);
                Vec::new()
            }
            Msg::Result(res) => {
                if self.in_detail() {
                    let ctx = Ctx {
                        now: self.now(),
                        zone: self.list.zone,
                        styles: &self.styles,
                    };
                    self.detail.accept_result(res, self.list.selected(), &ctx);
                } else {
                    self.detail.result.accept(res, None);
                }
                Vec::new()
            }
            Msg::Prompts(res) => {
                if self.in_detail() {
                    let ctx = Ctx {
                        now: self.now(),
                        zone: self.list.zone,
                        styles: &self.styles,
                    };
                    self.detail.accept_prompts(res, self.list.selected(), &ctx);
                }
                Vec::new()
            }
            Msg::Stopped(res) => {
                self.apply_stop(res);
                Vec::new()
            }
            Msg::Rejected(text) => {
                // Stop and open-log keys exist only on the detail screen;
                // the config window queues saves.
                if self.in_detail() {
                    self.detail.notice = text;
                } else if let Some(form) = self.config.as_mut()
                    && form.saving
                {
                    form.save_failed(&text);
                }
                Vec::new()
            }
            Msg::ConfigProbe(seq) => {
                if let Some(form) = self.config.as_mut()
                    && form.probe.seq == seq
                    && let Some(req) = form.probe_request()
                {
                    self.jobs.push(Cmd::Job(Job::Probe(Box::new(req))));
                }
                Vec::new()
            }
            Msg::ProxyModels(res) => {
                if let Some(form) = self.config.as_mut() {
                    form.probe_done(res.seq, res.result);
                }
                Vec::new()
            }
            Msg::CheckRow { run, row } => {
                if let Some(form) = self.config.as_mut() {
                    form.check_row(run, *row);
                }
                Vec::new()
            }
            Msg::CheckDone { run, report } => {
                let now = self.now();
                if let Some(form) = self.config.as_mut() {
                    form.check_done(run, *report, now);
                }
                Vec::new()
            }
            Msg::ConfigSaved(res) => self.config_saved(*res),
        }
    }

    // --- the config window --------------------------------------------------

    /// Whether the config window has a check running (the spinner keeps
    /// going for it).
    fn config_busy(&self) -> bool {
        self.config.as_ref().is_some_and(|f| f.check.running)
    }

    /// `c` in the list, and `rival config`: opens the window on the saved
    /// config and asks the proxy for its models.
    fn open_config(&mut self) {
        let Some(seed) = self.config_seed.clone() else {
            return;
        };
        self.config = Some(ConfigForm::new(seed));
        self.mode = Mode::Config;
        self.show_all_help = false;
        self.config_effect(Some(Effect::Probe { debounce: false }));
    }

    /// Leaves the window: back to the list, or out of `rival config`. A
    /// running check is cancelled; the seed keeps what was saved.
    fn close_config(&mut self) -> Vec<Cmd> {
        if let Some(mut form) = self.config.take() {
            form.cancel_check();
            self.config_seed = Some(form.seed);
        }
        if self.config_only {
            return self.quit();
        }
        self.mode = Mode::List;
        Vec::new()
    }

    fn config_key(&mut self, key: &str, ev: &KeyEvent) -> Vec<Cmd> {
        let Some(form) = self.config.as_mut() else {
            self.mode = Mode::List;
            return Vec::new();
        };
        let effect = form.apply(key, ev);
        if effect == Some(Effect::Close) {
            return self.close_config();
        }
        self.config_effect(effect);
        self.sync_config_mode();
        Vec::new()
    }

    /// The text edit owns the keyboard while it is open.
    fn sync_config_mode(&mut self) {
        if let Some(form) = &self.config {
            self.mode = if form.editing() {
                Mode::ConfigEdit
            } else {
                Mode::Config
            };
        }
    }

    /// Turns the window's effect into jobs and timers.
    fn config_effect(&mut self, effect: Option<Effect>) {
        let Some(form) = self.config.as_mut() else {
            return;
        };
        match effect {
            None | Some(Effect::Close) => {}
            Some(Effect::Probe { debounce: true }) => {
                let seq = form.bump_probe();
                self.jobs
                    .push(Cmd::After(PROBE_DEBOUNCE, Msg::ConfigProbe(seq)));
            }
            Some(Effect::Probe { debounce: false }) => {
                form.bump_probe();
                if let Some(req) = form.probe_request() {
                    self.jobs.push(Cmd::Job(Job::Probe(Box::new(req))));
                }
            }
            Some(Effect::Check { all }) => match form.start_check(all) {
                Ok(req) => {
                    self.jobs.push(Cmd::Job(Job::Check(Box::new(req))));
                    if !self.spinning {
                        self.spinning = true;
                        self.jobs.push(Cmd::Spin);
                    }
                }
                Err(e) => form.check_refused(&e),
            },
            Some(Effect::Save) => match form.save_request() {
                Ok(req) => self.jobs.push(Cmd::Job(Job::Save(req))),
                Err(e) => form.save_failed(&e),
            },
        }
    }

    fn config_saved(&mut self, res: SaveResult) -> Vec<Cmd> {
        let Some(form) = self.config.as_mut() else {
            return Vec::new();
        };
        match res {
            Ok(saved) => {
                if form.saved(&saved) {
                    return self.close_config();
                }
            }
            Err(e) => form.save_failed(&e),
        }
        Vec::new()
    }

    fn quit(&mut self) -> Vec<Cmd> {
        self.cancel_work();
        self.quitting = true;
        vec![Cmd::Quit]
    }

    /// Starts whichever of the refresh tick and the spinner is not already
    /// running, while anything is live. Each chain renews itself and ends on
    /// its own once nothing runs, so a second one would double its rate.
    fn ensure_live_timers(&mut self) -> Vec<Cmd> {
        let mut cmds = Vec::new();
        if !self.list.any_live {
            return cmds;
        }
        if !self.ticking {
            self.ticking = true;
            cmds.push(Cmd::Tick);
        }
        if !self.spinning {
            self.spinning = true;
            cmds.push(Cmd::Spin);
        }
        cmds
    }

    /// Rebuilds the list from a watcher snapshot. The list keeps the cursor
    /// on the same run; when that run vanished while its detail view was
    /// open, the user drops back to the list rather than seeing another
    /// run's log under the old heading.
    fn apply_sessions(&mut self, sessions: &[Arc<Session>]) {
        let anchor = self.list.selected_key();
        let now = self.now();
        self.list.set_items(group_sessions(sessions), now);
        if self.in_detail() {
            if self.list.selected_key() != anchor {
                self.close_detail();
            } else {
                self.sync_detail(false);
            }
        }
    }

    /// Whether the detail screen shows: browsing it, typing a search, or
    /// answering the stop confirm.
    pub fn in_detail(&self) -> bool {
        matches!(self.mode, Mode::Detail | Mode::Search | Mode::Confirm)
    }

    fn close_detail(&mut self) {
        self.mode = Mode::List;
        self.detail.close();
    }

    /// Resizes the detail viewport and reloads its content
    /// for the current selection. `reset` puts the scroll back to the tab's
    /// start (the tail for Raw); otherwise follow decides.
    fn sync_detail(&mut self, reset: bool) {
        if !self.in_detail() {
            return;
        }
        self.resize_detail();
        let ctx = Ctx {
            now: self.now(),
            zone: self.list.zone,
            styles: &self.styles,
        };
        if let Some(job) = self.detail.reload(self.list.selected(), &ctx, reset) {
            self.jobs.push(Cmd::Job(job));
        }
    }

    /// Fits the detail pane to the current geometry without re-reading.
    fn resize_detail(&mut self) {
        if self.in_detail() {
            self.detail.resize(self.lay.width, self.content_height());
        }
    }

    fn list_key(&mut self, key: &str) -> Vec<Cmd> {
        let k = self.keys;
        let now = self.now();
        if k.quit.matches(key) {
            return self.quit();
        } else if k.up.matches(key) {
            self.list.move_by(-1);
        } else if k.down.matches(key) {
            self.list.move_by(1);
        } else if k.top.matches(key) {
            self.list.top();
        } else if k.bottom.matches(key) {
            self.list.bottom();
        } else if k.next_page.matches(key) {
            self.list.turn_page(1);
        } else if k.prev_page.matches(key) {
            self.list.turn_page(-1);
        } else if k.next_tab.matches(key) {
            self.list.cycle_tab(1, now);
        } else if k.prev_tab.matches(key) {
            self.list.cycle_tab(-1, now);
        } else if k.help.matches(key) {
            self.show_all_help = !self.show_all_help;
        } else if k.filter.matches(key) {
            self.mode = Mode::Filter;
            self.list.filter.focus();
        } else if k.back.matches(key) {
            // esc outside the input clears a kept filter.
            if !self.list.filter.is_empty() {
                self.list.filter.reset();
                self.list.refilter(now);
            }
        } else if OPEN_CONFIG.matches(key) {
            self.open_config();
        } else if k.open.matches(key) && self.list.selected().is_some() {
            self.mode = Mode::Detail;
            if let Some(req) = self.detail.open(self.list.selected()) {
                self.jobs.push(Cmd::Job(Job::Prompts(req)));
            }
            // Open at the tail: live output and final verdicts both live at
            // the end of the log.
            self.sync_detail(true);
        }
        Vec::new()
    }

    fn filter_key(&mut self, key: &str, ev: &KeyEvent) -> Vec<Cmd> {
        let now = self.now();
        if self.keys.open.matches(key) {
            self.list.filter.blur();
            self.mode = Mode::List;
        } else if self.keys.back.matches(key) {
            self.list.filter.reset();
            self.list.filter.blur();
            self.list.refilter(now);
            self.mode = Mode::List;
        } else if ARROW_UP.matches(key) {
            self.list.move_by(-1);
        } else if ARROW_DOWN.matches(key) {
            self.list.move_by(1);
        } else {
            let before = self.list.filter.value();
            self.list.filter.handle_key(ev);
            if self.list.filter.value() != before {
                self.list.refilter(now);
            }
        }
        Vec::new()
    }

    /// Drives the detail screen while it has focus.
    fn detail_key(&mut self, key: &str) -> Vec<Cmd> {
        let k = self.keys;
        self.detail.notice.clear();
        if k.quit.matches(key) {
            return self.quit();
        } else if k.back.matches(key) || key == "backspace" {
            // esc peels one layer: a search first, then the screen.
            if !self.detail.query.is_empty() {
                self.detail.clear_search();
                self.detail.apply_content(&self.styles);
            } else {
                self.close_detail();
            }
        } else if k.help.matches(key) {
            self.show_all_help = !self.show_all_help;
        } else if let Some(tab) = k.detail_tab(key) {
            self.set_detail_tab(tab);
        } else if k.next_tab.matches(key) {
            self.set_detail_tab(self.detail.tab.cycle(1));
        } else if k.prev_tab.matches(key) {
            self.set_detail_tab(self.detail.tab.cycle(-1));
        } else if k.next_member.matches(key) || k.prev_member.matches(key) {
            // The view is not reset when the member stays; the app keeps
            // the reader's place, and so does this.
            let delta = if k.next_member.matches(key) { 1 } else { -1 };
            if self.detail.cycle_member(self.list.selected(), delta) {
                self.sync_detail(true);
            }
        } else if k.follow.matches(key) || k.bottom.matches(key) {
            self.detail.follow = true;
            self.detail.vp.goto_bottom();
        } else if k.top.matches(key) {
            self.detail.vp.goto_top();
            self.detail.follow = self.detail.vp.at_bottom();
        } else if k.search.matches(key) {
            self.mode = Mode::Search;
            let query = self.detail.query.clone();
            self.detail.search.set_value(&query);
            self.detail.search.cursor_end();
            self.detail.search.focus();
        } else if k.next_match.matches(key) {
            self.detail.step_match(1);
        } else if k.prev_match.matches(key) {
            self.detail.step_match(-1);
        } else if k.open_log.matches(key) {
            if let Some(item) = self.list.selected() {
                let req = if item.is_group() {
                    Some(OpenLogRequest {
                        sessions: item.sessions.clone(),
                        group: true,
                    })
                } else {
                    item.primary()
                        .filter(|s| !s.log_file.is_empty())
                        .map(|s| OpenLogRequest {
                            sessions: vec![Arc::clone(s)],
                            group: false,
                        })
                };
                if let Some(req) = req {
                    self.jobs.push(Cmd::Job(Job::OpenLog(req)));
                }
            }
        } else if k.stop.matches(key) {
            // Never signal on one key: x only opens the confirm bar.
            let item = self.list.selected();
            let targets = live_targets(item, self.alive);
            if !targets.is_empty() {
                self.detail.confirm = Some(KillConfirm { targets });
                self.mode = Mode::Confirm;
            } else if has_unverified(item) {
                self.detail.notice = UNVERIFIED_NOTICE.to_string();
            } else {
                self.detail.notice = "nothing running".to_string();
            }
        } else if self.detail.result_key(
            key,
            &k,
            self.list.selected(),
            &Ctx {
                now: self.now(),
                zone: self.list.zone,
                styles: &self.styles,
            },
        ) {
            // The Result tab moved its finding focus or opened a finding.
        } else {
            // Scroll keys (j/k/up/down/pgup/pgdown/space/u/d/b) belong to
            // the viewport. Scrolling away from the tail pauses follow;
            // scrolling back down to it resumes.
            self.detail.vp.handle_key(key);
            if self.detail.tab == DetailTab::Raw {
                self.detail.follow = self.detail.vp.at_bottom();
            }
        }
        Vec::new()
    }

    fn set_detail_tab(&mut self, tab: DetailTab) {
        self.detail.tab = tab;
        self.sync_detail(true);
    }

    /// Every key but enter, esc and ctrl+c is text, so "q" and "n" type
    /// themselves.
    fn search_key(&mut self, key: &str, ev: &KeyEvent) -> Vec<Cmd> {
        if self.keys.open.matches(key) {
            self.detail.search.blur();
            self.mode = Mode::Detail;
            let query = self.detail.search.value();
            self.detail.run_search(&query, &self.styles);
        } else if self.keys.back.matches(key) {
            self.detail.clear_search();
            self.detail.apply_content(&self.styles);
            self.mode = Mode::Detail;
        } else {
            self.detail.search.handle_key(ev);
        }
        Vec::new()
    }

    /// Handles a key while the stop bar is open. Every key closes the bar, and only y stops, so
    /// a stray key press can never kill a run. The targets are re-checked
    /// against the current snapshot first; the stop itself runs as a job,
    /// which checks each process identity again right before its signal.
    fn confirm_key(&mut self, key: &str) -> Vec<Cmd> {
        let confirm = self.detail.confirm.take();
        self.mode = Mode::Detail;
        let Some(confirm) = confirm.filter(|_| self.keys.yes.matches(key)) else {
            return Vec::new();
        };
        let item = self.list.selected();
        let targets = recheck_targets(item, &confirm.targets);
        if targets.is_empty() {
            self.detail.notice = "nothing running".to_string();
            return Vec::new();
        }
        let item_key = item.map(item_key).unwrap_or_default();
        self.jobs
            .push(Cmd::Job(Job::Stop(StopRequest { item_key, targets })));
        Vec::new()
    }

    // --- geometry -----------------------------------------------------------

    /// What the help bar depends on besides the width.
    fn help_state(&self) -> (Mode, bool, bool) {
        (self.mode, self.show_all_help, self.shows_findings())
    }

    /// Whether the detail screen shows a Result tab with findings, whose
    /// keys the help then lists.
    fn shows_findings(&self) -> bool {
        self.in_detail()
            && self.detail.tab == DetailTab::Result
            && !self.detail.findings.rows.is_empty()
    }

    /// The help lines for the current mode, "?" and width.
    fn help_view(&self) -> Vec<Line<'static>> {
        help_lines(
            &self.keys.help_for(self.mode, self.shows_findings()),
            self.show_all_help,
            self.lay.width,
            &self.styles,
        )
    }

    /// Stores how many rows the help bar takes. Call it whenever the mode,
    /// "?" or the width changes.
    fn measure_help(&mut self) {
        self.help_h = self.help_view().len();
    }

    /// The list pane height: the layout body minus any rows the expanded
    /// help borrows.
    pub(crate) fn list_body_height(&self) -> usize {
        self.lay
            .body_h
            .saturating_sub(self.help_h.saturating_sub(1))
            .max(1)
    }

    /// The list's size inside its border. The border exists only in the
    /// split view; alone, the list takes the whole body.
    pub(crate) fn list_inner_height(&self) -> usize {
        if self.lay.show_preview {
            return self.list_body_height().saturating_sub(2).max(1);
        }
        self.list_body_height()
    }

    #[cfg(test)]
    pub(crate) fn list_inner_width(&self) -> usize {
        if self.lay.show_preview {
            return self.lay.list_w.saturating_sub(2).max(1);
        }
        self.lay.width
    }

    /// How many data rows fit between the list's column titles and its page
    /// footer.
    pub(crate) fn list_rows(&self) -> usize {
        self.list_inner_height().saturating_sub(2).max(1)
    }

    /// The preview text width: the box minus its border and a 1-col pad each
    /// side.
    pub(crate) fn preview_inner_width(&self) -> usize {
        self.lay.preview_w.saturating_sub(4).max(1)
    }

    /// The detail body height: everything between the header and the help
    /// bar. The view and `sync_detail` both use it.
    pub(crate) fn content_height(&self) -> usize {
        self.lay
            .height
            .saturating_sub(self.lay.header_h + self.help_h)
    }

    // --- view ---------------------------------------------------------------

    /// The header's session counts.
    fn header_stats(&self) -> HeaderStats {
        HeaderStats {
            version: self.version.clone(),
            loading: !self.loaded,
            ..self.list.stats.clone()
        }
    }

    /// The current spinner frame, or "" when nothing runs.
    pub(crate) fn spin_frame(&self) -> &'static str {
        if self.loaded && !self.list.any_live {
            return "";
        }
        SPIN_FRAMES[self.spin_frame]
    }

    /// Draws the frame.
    pub fn draw(&self, frame: &mut Frame) {
        let area = frame.area();
        self.render(area, frame.buffer_mut());
    }

    /// Draws the frame into `area` of `buf`. The geometry comes from the
    /// last resize: the frame is `lay.width`×`lay.height`, clipped to
    /// `area`, so a stale size can never write out of bounds or past the
    /// frame it laid out.
    pub fn render(&self, area: Rect, buf: &mut Buffer) {
        let area = area.intersection(buf.area);
        if area.is_empty() || self.quitting {
            return;
        }
        if !self.err_text.is_empty() {
            Rows::new(area, buf).line(Line::raw(format!("Error: {}", self.err_text)));
            return;
        }
        if self.lay.width == 0 || self.lay.height == 0 {
            Rows::new(area, buf).line(Line::raw("Initializing..."));
            return;
        }
        let to_u16 = |n: usize| u16::try_from(n).unwrap_or(u16::MAX);
        let area = Rect {
            width: area.width.min(to_u16(self.lay.width)),
            height: area.height.min(to_u16(self.lay.height)),
            ..area
        };
        let mut out = Rows::new(area, buf);
        let w = self.lay.width;
        if self.lay.too_small {
            let notice = format!("terminal too small (need {MIN_WIDTH}×{MIN_HEIGHT})");
            out.line(Line::raw(truncate(&notice, w, "")));
            return;
        }

        if let Some(form) = self.config.as_ref().filter(|_| self.in_config()) {
            let ctx = ViewCtx {
                styles: &self.styles,
                spin: SPIN_FRAMES[self.spin_frame],
            };
            config_view::render(form, area, out.buf, &ctx);
            return;
        }
        let spin = self.spin_frame();
        for line in render_header(
            w,
            self.lay.compact,
            &self.header_stats(),
            spin,
            &self.styles,
        ) {
            out.line(pad_line(line, w));
        }
        if self.in_detail() {
            let h = self.content_height();
            let rect = out.take(h);
            self.detail
                .render(self.list.selected(), rect, out.buf, spin, &self.ctx());
        } else {
            out.line(self.list.tab_bar(w, !self.loaded, &self.styles));
            let rect = out.take(self.list_body_height());
            if self.loaded {
                self.render_body(rect, out.buf, spin);
            } else {
                self.render_loader(rect, out.buf, spin);
            }
        }
        for line in self.help_view() {
            out.line(line);
        }
    }

    /// The list alone, or from the preview width up the list and the
    /// preview in rounded boxes with a 1-col gap. The list has focus here,
    /// so its border is accent and the preview's is dim.
    fn render_body(&self, area: Rect, buf: &mut Buffer, spin: &str) {
        let now = self.now();
        if !self.lay.show_preview {
            self.list.render(area, buf, spin, now, &self.styles);
            return;
        }
        let list_w = u16::try_from(self.lay.list_w).unwrap_or(u16::MAX);
        let left = Rect {
            width: list_w.min(area.width),
            ..area
        };
        let gap = left.width.saturating_add(1).min(area.width);
        let right = Rect {
            x: area.x + gap,
            width: area.width - gap,
            ..area
        };
        let list_box = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(self.styles.focus_border);
        let inner = list_box.inner(left);
        list_box.render(left, buf);
        self.list.render(inner, buf, spin, now, &self.styles);
        let preview_box = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(self.styles.border)
            .padding(Padding::horizontal(1));
        let inner = preview_box.inner(right);
        preview_box.render(right, buf);
        self.preview
            .render(self.list.selected(), inner, buf, &self.ctx());
    }

    /// Fills the list body until the first snapshot: the spinner and
    /// "reading sessions done/total" over a bar in the logo gradient,
    /// centred in the body.
    fn render_loader(&self, area: Rect, buf: &mut Buffer, spin: &str) {
        let (w, h) = (usize::from(area.width), usize::from(area.height));
        if w == 0 || h == 0 {
            return;
        }
        let mut label = "reading sessions".to_string();
        let mut pct = 0.0;
        if self.load.total > 0 {
            label.push_str(&format!(" {}/{}", self.load.done, self.load.total));
            pct = self.load.done as f64 / self.load.total as f64;
        }
        let bar_w = LOADER_BAR_MAX.min(w.saturating_sub(4).max(1));
        let title = Line::from(vec![
            Span::styled(spin.to_string(), self.styles.running),
            Span::raw(" "),
            Span::styled(label, self.styles.text),
        ]);
        let mut block = vec![fit_line(title, bar_w)];
        if h >= 3 {
            block.push(Line::default());
            block.push(gradient_bar(bar_w, pct, &self.styles));
        }
        let top = (h - block.len().min(h)) / 2;
        let left = (w - bar_w.min(w)) / 2;
        for (i, line) in block.into_iter().enumerate().take(h - top) {
            // Centre each line inside the bar's width, then the bar in the
            // body.
            let pad = (bar_w - line_width(&line).min(bar_w)) / 2;
            let x = area.x + u16::try_from(left + pad).unwrap_or(0);
            let y = area.y + u16::try_from(top + i).unwrap_or(0);
            buf.set_line(x, y, &line, area.width.saturating_sub(x - area.x));
        }
    }
}

/// Writes whole rows top-down into an area and never past its bottom.
struct Rows<'a> {
    area: Rect,
    y: u16,
    buf: &'a mut Buffer,
}

impl<'a> Rows<'a> {
    fn new(area: Rect, buf: &'a mut Buffer) -> Rows<'a> {
        Rows { area, y: 0, buf }
    }

    /// Draws one line in the next row.
    fn line(&mut self, line: Line<'static>) {
        let rect = self.take(1);
        if rect.height > 0 {
            self.buf.set_line(rect.x, rect.y, &line, rect.width);
        }
    }

    /// Reserves the next `n` rows, clipped to the area.
    fn take(&mut self, n: usize) -> Rect {
        let n = u16::try_from(n).unwrap_or(u16::MAX);
        let height = n.min(self.area.height - self.y);
        let rect = Rect {
            x: self.area.x,
            y: self.area.y + self.y,
            width: self.area.width,
            height,
        };
        self.y += height;
        rect
    }
}

#[cfg(test)]
mod config_tests;
#[cfg(test)]
mod detail_tests;
#[cfg(test)]
mod list_tests;
#[cfg(test)]
mod result_tests;
#[cfg(test)]
mod tests;
