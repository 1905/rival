//! The terminal runtime: `rival tui`. Go: `cmd/tui.go` plus bubbletea's
//! `Program.Run`.
//!
//! [`run`] owns everything the [`Model`] must not touch: the terminal, the
//! signals, the session watcher, the timers, the job workers and the opened
//! log copies. Every source feeds one channel of [`Event`]s, and
//! [`event_loop`] turns them into model updates and frames. On every exit
//! (quit, signal, error or unwind) the scoped owners stop their threads and
//! the terminal is restored.

use std::collections::VecDeque;
use std::io::{self, IsTerminal};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossterm::event::{self as term_event, Event as TermEvent};
use crossterm::{cursor, execute, terminal};
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::layout::Rect;
use ratatui::{Terminal, TerminalOptions, Viewport};

use rival_core::cancel::{CancelFunc, Context};
use rival_core::config::Config;
use rival_core::paths::Paths;
use rival_core::sessionview::{self, SessionWatcher};

use super::jobs::{Job, JobEnv, JobOutput, LogViews};
use super::logview::LogPane;
use super::model::{Cmd, Model, Msg, SPIN_INTERVAL, TICK_INTERVAL};
use crate::signals::{self, Signal};

/// What an OS SIGINT makes `Program.Run` return: `ErrProgramKilled`
/// wrapping `ErrInterrupted`. A raw ctrl+c is a key and quits normally.
pub const INTERRUPTED: &str = "program was killed: program was interrupted";

/// How many workers run [`Job`]s. Fixed, so a burst of jobs queues instead
/// of starting threads.
pub const JOB_WORKERS: usize = 2;

/// How often the runtime reaps finished viewer launchers and expires log
/// copies. It runs whether or not the model ticks: the model's tick and
/// spinner chains stop once nothing is live.
pub const SWEEP_INTERVAL: Duration = Duration::from_secs(1);

/// How long the input thread waits for a key before it checks for shutdown.
const INPUT_POLL: Duration = Duration::from_millis(50);

/// The most events handled before the next frame is drawn.
const BATCH: usize = 64;

/// Where the frame goes when stdout is not a terminal. Go then has no size
/// (0×0), so the model shows "Initializing..."; one row holds it.
const NO_TTY_AREA: Rect = Rect::new(0, 0, 80, 1);

/// Everything that reaches the loop.
#[derive(Debug)]
pub enum Event {
    /// A key, paste or resize from the terminal.
    Term(TermEvent),
    /// The terminal could not be read any more.
    InputFailed(String),
    /// A watcher snapshot, scan progress or start error.
    Msg(Msg),
    /// A finished job.
    Job(JobOutput),
    /// SIGINT or SIGTERM.
    Signal(Signal),
}

/// Go `tea.NewProgram(dashboard.New()).Run()`. Returns the error text
/// after "tui: ".
pub fn run(cfg: &Config) -> Result<(), String> {
    check_input()?;
    let (tx, rx) = mpsc::channel();
    // Before raw mode, as bubbletea installs its handlers first: SIGINT and
    // SIGTERM must never take their default action over a raw terminal.
    let _signals = {
        let tx = tx.clone();
        signals::notify(move |sig| {
            let _ = tx.send(Event::Signal(sig));
        })
        .map_err(|e| format!("install signal handlers: {e}"))?
    };
    let resize = io::stdout().is_terminal();
    // From here every exit (return, `?` or unwind) runs `Owners::close`.
    let mut owners = Owners::new(rx);
    owners.screen = Some(ScreenGuard::enter(TerminalScreen::default())?);
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = if resize {
        Terminal::new(backend)
            .map_err(|e| format!("bubbletea: error getting terminal size: {e}"))?
    } else {
        Terminal::with_options(
            backend,
            TerminalOptions {
                viewport: Viewport::Fixed(NO_TTY_AREA),
            },
        )
        .map_err(|e| e.to_string())?
    };
    let size = if resize {
        terminal.size().map_err(|e| e.to_string())?
    } else {
        Default::default()
    };
    let _ = tx.send(Event::Msg(Msg::Resize {
        width: size.width,
        height: size.height,
    }));

    let mut model = Model::new(rival_core::VERSION);
    let paths = cfg.paths().clone();
    let spawn_err = |what: &str, e: io::Error| format!("start {what}: {e}");
    let jobs = JobPool::start(JobEnv::system(paths.clone()), JOB_WORKERS, tx.clone())
        .map_err(|e| spawn_err("job workers", e))?;
    let jobs = owners.jobs.insert(jobs);
    owners.watch =
        Some(Watch::start(paths, tx.clone()).map_err(|e| spawn_err("session watch", e))?);
    owners.input =
        Some(Input::start(TERMINAL_INPUT, tx.clone()).map_err(|e| spawn_err("terminal input", e))?);
    drop(tx);

    let opts = LoopOpts {
        resize,
        sweep_every: SWEEP_INTERVAL,
        clock: Instant::now,
    };
    let result = event_loop(
        &mut model,
        &mut terminal,
        &owners.events,
        jobs,
        &mut owners.views,
        &opts,
    );
    drop(terminal);
    owners.close();
    result
}

/// Everything [`run`] starts, closed in one fixed order on every exit:
/// normal return, a startup error after the screen was entered, or an
/// unwind. Keys stop first, so nothing reads the terminal once it is back
/// in cooked mode; the screen is restored next, before anything that can
/// block (the initial scan, a running job); then the watcher, the job
/// workers and the log copies wind down.
struct Owners<S: Screen = TerminalScreen> {
    input: Option<Input>,
    screen: Option<ScreenGuard<S>>,
    watch: Option<Watch>,
    jobs: Option<JobPool>,
    views: LogViews,
    events: Receiver<Event>,
}

impl<S: Screen> Owners<S> {
    fn new(events: Receiver<Event>) -> Owners<S> {
        Owners {
            input: None,
            screen: None,
            watch: None,
            jobs: None,
            views: LogViews::default(),
            events,
        }
    }

    fn close(&mut self) {
        if let Some(mut input) = self.input.take() {
            input.stop();
        }
        drop(self.screen.take());
        if let Some(mut watch) = self.watch.take() {
            watch.stop();
        }
        if let Some(mut jobs) = self.jobs.take() {
            finish_jobs(&mut jobs, &self.events, &mut self.views);
        }
        self.views.close();
    }
}

impl<S: Screen> Drop for Owners<S> {
    fn drop(&mut self) {
        self.close();
    }
}

/// Go `Program.Run`: when stdin is not a terminal, bubbletea reads keys
/// from `/dev/tty` instead, and fails only when that cannot be opened.
/// Crossterm's `use-dev-tty` reader falls back to `/dev/tty` the same way
/// (its default mio reader cannot register that fd on macOS); this only
/// gives the failure Go's text.
#[cfg(unix)]
fn check_input() -> Result<(), String> {
    if io::stdin().is_terminal() {
        return Ok(());
    }
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .map(drop)
        .map_err(|e| {
            format!(
                "bubbletea: error opening TTY: bubbletea: could not open TTY: open /dev/tty: {}",
                e
            )
        })
}

/// Windows needs no separate preflight. bubbletea opens `CONIN$` when stdin
/// is not a terminal; crossterm 0.29 already reads the console input
/// buffer, not stdin: `enable_raw_mode` and its event source open `CONIN$`
/// through crossterm_winapi 0.9.1 `Handle::current_in_handle`. A process
/// without a console fails there, in `ScreenGuard::enter`, with crossterm's
/// error text rather than bubbletea's `could not open TTY` wording.
#[cfg(not(unix))]
fn check_input() -> Result<(), String> {
    Ok(())
}

/// After the loop: jobs still queued run first when they matter (a
/// confirmed stop, an "o"); an opened copy that never reached the loop
/// joins the others, so every copy follows the same exit policy.
fn finish_jobs(jobs: &mut JobPool, events: &Receiver<Event>, views: &mut LogViews) {
    jobs.shutdown();
    for ev in events.try_iter() {
        if let Event::Job(JobOutput::Opened(opened)) = ev {
            views.adopt(opened);
        }
    }
    views.close();
}

// --- the loop -----------------------------------------------------------------

/// What the loop needs besides its inputs.
pub struct LoopOpts {
    /// Whether terminal resizes reach the model. False when stdout is not a
    /// terminal: Go then never learns a size.
    pub resize: bool,
    /// How often [`LogViews::sweep`] runs.
    pub sweep_every: Duration,
    /// The clock for timers and the sweep; tests move it.
    pub clock: fn() -> Instant,
}

/// Runs the model until it quits (`Ok`), a SIGTERM arrives (`Ok`, as Go's
/// `QuitMsg`), a SIGINT arrives (`Err(INTERRUPTED)`) or the input fails.
/// It draws after every batch of events, starts the model's tick and
/// spinner timers, sends its jobs to `jobs`, and gives opened log copies to
/// `views`, which it sweeps every `opts.sweep_every`.
pub fn event_loop<B: Backend>(
    model: &mut Model,
    terminal: &mut Terminal<B>,
    events: &Receiver<Event>,
    jobs: &JobPool,
    views: &mut LogViews,
    opts: &LoopOpts,
) -> Result<(), String> {
    let mut timers = Timers::default();
    let mut next_sweep = (opts.clock)() + opts.sweep_every;
    let mut rejected = Vec::new();
    let init = model.init();
    if let Some(end) = apply(init, &mut timers, jobs, (opts.clock)(), &mut rejected) {
        return end;
    }
    for msg in rejected {
        if let Some(end) = step(model, msg, &mut timers, jobs, opts.clock) {
            return end;
        }
    }
    loop {
        let _ = terminal.draw(|frame| model.draw(frame));
        let now = (opts.clock)();
        let deadline = timers.next().map_or(next_sweep, |t| t.min(next_sweep));
        let mut next = match events.recv_timeout(deadline.saturating_duration_since(now)) {
            Ok(ev) => Some(ev),
            Err(RecvTimeoutError::Timeout) => None,
            // Every sender is gone: nothing can wake the loop again.
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        };
        let mut handled = 0;
        while let Some(ev) = next.take() {
            let msg = match route(ev, views, opts.resize) {
                Route::Msg(msg) => msg,
                Route::Done => None,
                Route::End(end) => return end,
            };
            if let Some(msg) = msg
                && let Some(end) = step(model, msg, &mut timers, jobs, opts.clock)
            {
                return end;
            }
            handled += 1;
            if handled < BATCH {
                next = events.try_recv().ok();
            }
        }
        let now = (opts.clock)();
        if now >= next_sweep {
            views.sweep(now);
            next_sweep = now + opts.sweep_every;
        }
        for msg in timers.take_due(now) {
            if let Some(end) = step(model, msg, &mut timers, jobs, opts.clock) {
                return end;
            }
        }
    }
}

/// What the model hears when the job queue cannot take a stop or a log
/// open. Rust-only: Go ran both inside its update.
pub const BUSY_NOTICE: &str = "busy: earlier stops or log opens still queued, try again";

/// Feeds `msg` to the model and carries out its commands. A job the pool
/// refuses comes back as [`Msg::Rejected`], so it never vanishes unseen.
/// `Some` ends the loop.
fn step(
    model: &mut Model,
    msg: Msg,
    timers: &mut Timers,
    jobs: &JobPool,
    clock: fn() -> Instant,
) -> Option<Result<(), String>> {
    let mut queue = vec![msg];
    while let Some(msg) = queue.pop() {
        let cmds = model.update(msg);
        if let Some(end) = apply(cmds, timers, jobs, clock(), &mut queue) {
            return Some(end);
        }
    }
    None
}

enum Route {
    /// Feed this to the model (or nothing).
    Msg(Option<Msg>),
    /// Handled here.
    Done,
    /// Leave the loop with this result.
    End(Result<(), String>),
}

fn route(ev: Event, views: &mut LogViews, resize: bool) -> Route {
    match ev {
        Event::Term(TermEvent::Key(key)) => Route::Msg(Some(Msg::Key(key))),
        Event::Term(TermEvent::Paste(text)) => Route::Msg(Some(Msg::Paste(text))),
        Event::Term(TermEvent::Resize(width, height)) if resize => {
            Route::Msg(Some(Msg::Resize { width, height }))
        }
        Event::Term(_) => Route::Done,
        Event::InputFailed(e) => Route::End(Err(format!("program was killed: {e}"))),
        Event::Msg(msg) => Route::Msg(Some(msg)),
        Event::Job(JobOutput::Msg(msg)) => Route::Msg(Some(msg)),
        Event::Job(JobOutput::Opened(opened)) => {
            views.adopt(opened);
            Route::Done
        }
        Event::Job(JobOutput::Nothing) => Route::Done,
        // Bubbletea: SIGINT → InterruptMsg → ErrInterrupted; SIGTERM →
        // QuitMsg, a normal return.
        Event::Signal(Signal::Interrupt) => Route::End(Err(INTERRUPTED.to_string())),
        Event::Signal(Signal::Terminate) => Route::End(Ok(())),
    }
}

/// Carries out the model's commands; `Some` ends the loop. A refused job
/// adds [`Msg::Rejected`] to `rejected`.
fn apply(
    cmds: Vec<Cmd>,
    timers: &mut Timers,
    jobs: &JobPool,
    now: Instant,
    rejected: &mut Vec<Msg>,
) -> Option<Result<(), String>> {
    for cmd in cmds {
        match cmd {
            Cmd::Quit => return Some(Ok(())),
            Cmd::Tick => timers.add(now + TICK_INTERVAL, Msg::Tick),
            Cmd::Spin => timers.add(now + SPIN_INTERVAL, Msg::SpinTick),
            Cmd::Job(job) => {
                if jobs.submit(job).is_err() {
                    rejected.push(Msg::Rejected(BUSY_NOTICE.to_string()));
                }
            }
        }
    }
    None
}

/// The model's pending tick and spinner messages. The model keeps at most
/// one of each in flight.
#[derive(Default)]
struct Timers {
    pending: Vec<(Instant, Msg)>,
}

impl Timers {
    fn add(&mut self, at: Instant, msg: Msg) {
        self.pending.push((at, msg));
    }

    fn next(&self) -> Option<Instant> {
        self.pending.iter().map(|(at, _)| *at).min()
    }

    fn take_due(&mut self, now: Instant) -> Vec<Msg> {
        let (due, later): (Vec<_>, Vec<_>) = std::mem::take(&mut self.pending)
            .into_iter()
            .partition(|(at, _)| *at <= now);
        self.pending = later;
        due.into_iter().map(|(_, msg)| msg).collect()
    }
}

// --- job workers ----------------------------------------------------------------

/// How many stops and log opens may wait for a worker. One more is refused
/// with [`BUSY_NOTICE`] instead of queued.
pub const MAX_PENDING_ACTIONS: usize = 8;

/// The most jobs [`JobPool`] keeps waiting: one read per pane, one result
/// parse, one prompt load and the actions. Tests check the bound; `Pending`
/// keeps it by shape.
#[cfg(test)]
pub const MAX_PENDING: usize = 4 + MAX_PENDING_ACTIONS;

/// Work waiting for a worker, bounded by `MAX_PENDING`.
///
/// - A log read replaces the read still waiting for the same pane. The
///   model's `LogSlot` waits only for its newest request, and drops older
///   results anyway, so the replaced read loses nothing; the newest one
///   always stays and is delivered.
/// - A result parse replaces the waiting one, for the same reason: the
///   `ResultSlot` waits only for its newest request.
/// - A prompt load replaces the waiting one; the detail screen waits only
///   for the run it shows.
/// - Stops and log opens queue in order. One equal to a waiting job merges
///   with it (that one gives the outcome). Past [`MAX_PENDING_ACTIONS`] the
///   job is refused, and the caller tells the user.
#[derive(Debug, Default)]
struct Pending {
    preview: Option<Job>,
    detail: Option<Job>,
    result: Option<Job>,
    prompts: Option<Job>,
    actions: VecDeque<Job>,
    closing: bool,
}

impl Pending {
    #[cfg(test)]
    fn len(&self) -> usize {
        [&self.preview, &self.detail, &self.result, &self.prompts]
            .iter()
            .filter(|j| j.is_some())
            .count()
            + self.actions.len()
    }

    fn push(&mut self, job: Job) -> Result<(), Refused> {
        match &job {
            Job::Log(req) => {
                let slot = match req.pane {
                    LogPane::Preview => &mut self.preview,
                    LogPane::Detail => &mut self.detail,
                };
                *slot = Some(job);
            }
            Job::Result(_) => self.result = Some(job),
            Job::Prompts(_) => self.prompts = Some(job),
            Job::Stop(_) | Job::OpenLog(_) => {
                if self.actions.contains(&job) {
                    return Ok(());
                }
                if self.actions.len() >= MAX_PENDING_ACTIONS {
                    return Err(Refused);
                }
                self.actions.push_back(job);
            }
        }
        Ok(())
    }

    /// The next job: confirmed actions first, then the open run's read and
    /// parse, the preview's read and the prompts. Once closing, actions
    /// only.
    fn pop(&mut self) -> Option<Job> {
        if let Some(job) = self.actions.pop_front() {
            return Some(job);
        }
        if self.closing {
            return None;
        }
        self.detail
            .take()
            .or_else(|| self.result.take())
            .or_else(|| self.preview.take())
            .or_else(|| self.prompts.take())
    }
}

/// [`JobPool::submit`] could not queue the job: the action queue is full,
/// or the pool is closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Refused;

struct Shared {
    pending: Mutex<Pending>,
    wake: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Pending> {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A fixed set of workers running [`Job`]s with [`JobEnv::run`] over a
/// bounded [`Pending`] set. [`JobPool::submit`] only takes a short lock: it
/// never blocks the UI on a slow job.
pub struct JobPool {
    shared: Arc<Shared>,
    workers: Vec<JoinHandle<()>>,
}

impl JobPool {
    /// Starts `workers` threads. Every output goes to `out` as
    /// [`Event::Job`]. A failed spawn closes the pool before the error
    /// returns.
    pub fn start(env: JobEnv, workers: usize, out: Sender<Event>) -> io::Result<JobPool> {
        let mut pool = JobPool {
            shared: Arc::new(Shared {
                pending: Mutex::new(Pending::default()),
                wake: Condvar::new(),
            }),
            workers: Vec::new(),
        };
        for i in 0..workers {
            let (env, shared, out) = (env.clone(), Arc::clone(&pool.shared), out.clone());
            let worker = thread::Builder::new()
                .name(format!("rival-tui-job-{i}"))
                .spawn(move || work(&env, &shared, &out))?;
            pool.workers.push(worker);
        }
        Ok(pool)
    }

    /// Queues `job`, or refuses it when the action queue is full or the
    /// pool is closed.
    pub fn submit(&self, job: Job) -> Result<(), Refused> {
        let mut pending = self.shared.lock();
        if pending.closing {
            return Err(Refused);
        }
        pending.push(job)?;
        drop(pending);
        self.shared.wake.notify_one();
        Ok(())
    }

    /// How many jobs wait for a worker.
    #[cfg(test)]
    pub fn pending(&self) -> usize {
        self.shared.lock().len()
    }

    /// Lets the workers finish the waiting stops and log opens, then joins
    /// them. Waiting reads, parses and prompt loads are dropped: no screen
    /// is left to show them. Stops and log opens still run, as Go ran them
    /// inside the update before the quit key was read.
    pub fn shutdown(&mut self) {
        {
            let mut pending = self.shared.lock();
            pending.closing = true;
            pending.preview = None;
            pending.detail = None;
            pending.result = None;
            pending.prompts = None;
        }
        self.shared.wake.notify_all();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

impl Drop for JobPool {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn work(env: &JobEnv, shared: &Shared, out: &Sender<Event>) {
    loop {
        let job = {
            let mut pending = shared.lock();
            loop {
                if let Some(job) = pending.pop() {
                    break job;
                }
                if pending.closing {
                    return;
                }
                pending = shared
                    .wake
                    .wait(pending)
                    .unwrap_or_else(PoisonError::into_inner);
            }
        };
        // A refused send drops the output here; an opened copy then applies
        // its own exit policy.
        let _ = out.send(Event::Job(env.run(job)));
    }
}

// --- session watch --------------------------------------------------------------

/// Go `Init`'s watch command. The initial scan runs off the UI thread; its
/// progress, every snapshot and a start failure reach the loop as
/// messages. The runtime owns the cancel and every thread: [`Watch::stop`]
/// (also run on drop) cancels the watch, stops the watcher and joins them.
pub struct Watch {
    cancel: CancelFunc,
    /// The running watcher, once the start thread has it.
    watcher: Arc<Mutex<Option<SessionWatcher>>>,
    threads: Vec<JoinHandle<()>>,
}

/// A thread body.
pub type Body = Box<dyn FnOnce() + Send>;

/// Starts a named thread. Tests inject failures.
pub type Spawn = fn(&str, Body) -> io::Result<JoinHandle<()>>;

fn spawn_named(name: &str, body: Body) -> io::Result<JoinHandle<()>> {
    thread::Builder::new()
        .name(format!("rival-tui-{name}"))
        .spawn(body)
}

impl Watch {
    pub fn start(paths: Paths, out: Sender<Event>) -> io::Result<Watch> {
        Watch::start_with(paths, out, spawn_named)
    }

    /// [`Watch::start`] with an injected thread spawner. Every body is built
    /// before the first spawn. When a spawn fails, the bodies not started
    /// yet drop first, and with them the last channel senders, so the
    /// threads already running see their input close; only then are they
    /// joined.
    pub fn start_with(paths: Paths, out: Sender<Event>, spawn: Spawn) -> io::Result<Watch> {
        let (ctx, cancel) = Context::background().with_cancel();
        // Go: make(chan SessionEvent, 10) and make(chan LoadProgress, 1).
        let (events_tx, events) = mpsc::sync_channel(10);
        let (progress_tx, progress) = mpsc::sync_channel(1);
        let mut watch = Watch {
            cancel,
            watcher: Arc::new(Mutex::new(None)),
            threads: Vec::new(),
        };
        let found = Arc::clone(&watch.watcher);
        let starter_out = out.clone();
        let bodies: Vec<(&str, Body)> = vec![
            ("sessions", forward(events, out.clone(), Msg::Sessions)),
            ("progress", forward(progress, out, Msg::Progress)),
            (
                "watch-start",
                Box::new(move || {
                    match sessionview::watch_sessions(&ctx, &paths, events_tx, Some(progress_tx)) {
                        Ok(watcher) => {
                            *found.lock().unwrap_or_else(PoisonError::into_inner) = Some(watcher);
                        }
                        Err(err) => {
                            // Go `errMsg{err}`. A start cut short by the
                            // exit has nobody to show it to.
                            if !ctx.is_done() {
                                let msg = Msg::Error(err.to_string());
                                let _ = starter_out.send(Event::Msg(msg));
                            }
                        }
                    }
                }),
            ),
        ];
        let mut bodies = bodies.into_iter();
        while let Some((name, body)) = bodies.next() {
            match spawn(name, body) {
                Ok(thread) => watch.threads.push(thread),
                Err(e) => {
                    drop(bodies);
                    watch.stop();
                    return Err(e);
                }
            }
        }
        Ok(watch)
    }

    /// Cancels the watch and joins every thread. The initial scan cannot be
    /// interrupted, so this waits for it; its send then sees the cancel.
    /// The forwarders end once their senders are gone: the watcher worker
    /// holds the event sender, the scan the progress sender.
    pub fn stop(&mut self) {
        self.cancel.cancel();
        let mut threads = std::mem::take(&mut self.threads);
        // The start thread, when it started, is the last one: join it, then
        // stop the watcher it left, so the forwarders' senders are gone.
        // After a partial start no sender is left, so any order works.
        if let Some(last) = threads.pop() {
            let _ = last.join();
        }
        let watcher = self
            .watcher
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(watcher) = watcher {
            watcher.stop();
        }
        for thread in threads {
            let _ = thread.join();
        }
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.stop();
    }
}

fn forward<T: Send + 'static>(from: Receiver<T>, out: Sender<Event>, wrap: fn(T) -> Msg) -> Body {
    Box::new(move || {
        for value in from {
            if out.send(Event::Msg(wrap(value))).is_err() {
                return;
            }
        }
    })
}

// --- terminal input -------------------------------------------------------------

/// Where keys come from. Production reads crossterm's event queue, whose
/// `use-dev-tty` reader uses `/dev/tty` when stdin is not a terminal; tests
/// inject fakes.
#[derive(Clone, Copy)]
pub struct InputSource {
    pub poll: fn(Duration) -> io::Result<bool>,
    pub read: fn() -> io::Result<TermEvent>,
}

pub const TERMINAL_INPUT: InputSource = InputSource {
    poll: term_event::poll,
    read: term_event::read,
};

/// The input thread. It polls in short steps so [`Input::stop`] (also run
/// on drop) can end it without a pending read.
pub struct Input {
    stop: Arc<AtomicBool>,
    reader: Option<JoinHandle<()>>,
}

impl Input {
    pub fn start(source: InputSource, out: Sender<Event>) -> io::Result<Input> {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let reader = thread::Builder::new()
            .name("rival-tui-input".into())
            .spawn(move || read_input(source, &flag, &out))?;
        Ok(Input {
            stop,
            reader: Some(reader),
        })
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

impl Drop for Input {
    fn drop(&mut self) {
        self.stop();
    }
}

fn read_input(source: InputSource, stop: &AtomicBool, out: &Sender<Event>) {
    while !stop.load(Ordering::Acquire) {
        match (source.poll)(INPUT_POLL) {
            Ok(false) => continue,
            Ok(true) => {}
            Err(e) => {
                let _ = out.send(Event::InputFailed(e.to_string()));
                return;
            }
        }
        match (source.read)() {
            Ok(ev) => {
                if out.send(Event::Term(ev)).is_err() {
                    return;
                }
            }
            Err(e) => {
                let _ = out.send(Event::InputFailed(e.to_string()));
                return;
            }
        }
    }
}

// --- the screen -----------------------------------------------------------------

/// The terminal modes the TUI changes. `leave` undoes only what `enter`
/// did, and doing it twice is harmless.
pub trait Screen {
    fn enter(&mut self) -> Result<(), String>;
    fn leave(&mut self);
}

/// Holds the screen while the TUI runs. Dropping it (normal exit, error or
/// unwind) leaves it. A failed `enter` is undone before the error returns.
pub struct ScreenGuard<S: Screen> {
    screen: S,
}

impl<S: Screen> ScreenGuard<S> {
    pub fn enter(screen: S) -> Result<ScreenGuard<S>, String> {
        let mut guard = ScreenGuard { screen };
        guard.screen.enter()?;
        Ok(guard)
    }
}

impl<S: Screen> Drop for ScreenGuard<S> {
    fn drop(&mut self) {
        self.screen.leave();
    }
}

/// Raw mode, the alternate screen, a hidden cursor and bracketed paste.
#[derive(Debug, Default)]
pub struct TerminalScreen {
    raw: bool,
    alt: bool,
}

impl Screen for TerminalScreen {
    fn enter(&mut self) -> Result<(), String> {
        terminal::enable_raw_mode().map_err(|e| format!("error entering raw mode: {}", e))?;
        self.raw = true;
        self.alt = true;
        // Go ignores renderer write errors too.
        let _ = execute!(
            io::stdout(),
            terminal::EnterAlternateScreen,
            cursor::Hide,
            term_event::EnableBracketedPaste
        );
        Ok(())
    }

    fn leave(&mut self) {
        if std::mem::take(&mut self.alt) {
            let _ = execute!(
                io::stdout(),
                term_event::DisableBracketedPaste,
                cursor::Show,
                terminal::LeaveAlternateScreen
            );
        }
        if std::mem::take(&mut self.raw) {
            let _ = terminal::disable_raw_mode();
        }
    }
}

#[cfg(test)]
mod tests;
