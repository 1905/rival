//! Three real OS processes (A, B, C) compete for a single queue slot and must
//! hold it one at a time, in enqueue order. Each child is this test binary
//! re-run on the same test in helper mode (the standard subprocess-test
//! idiom), so this exercises the file lock across processes, not threads.
//!
//! No fixed delays: readiness is observed. The parent starts the next helper
//! only once the previous helper's ticket file exists, and the first holder
//! keeps the slot until every helper has enqueued. Each helper appends `ACQUIRED` and
//! `RELEASED` lines (one `O_APPEND` write each) to a shared events file while
//! it holds the slot, so the file order is the real order.

use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use super::Manager;
use crate::cancel::Context;

const HELPER_ENV: &str = "RIVAL_QUEUE_HELPER";
const TEST_NAME: &str = "queue::crossproc_tests::cross_process_fifo";
/// Every wait in the test and in the helpers is bounded by this.
const BOUND: Duration = Duration::from_secs(30);

/// Kills and reaps every helper still running when the test ends, so a
/// failing assertion cannot leave processes behind.
struct Helpers {
    root: PathBuf,
    children: Vec<(String, Child)>,
}

impl Helpers {
    fn output(&self, label: &str) -> String {
        read(&self.root.join(format!("{label}.out")))
    }

    fn outputs(&self) -> String {
        self.children
            .iter()
            .map(|(label, _)| format!("{label}:\n{}", self.output(label)))
            .collect()
    }

    /// Waits until `ready`, failing at once if a helper exits early.
    fn wait_until(&mut self, what: &str, mut ready: impl FnMut() -> bool) {
        let end = Instant::now() + BOUND;
        while !ready() {
            for (label, child) in &mut self.children {
                if let Some(status) = child.try_wait().unwrap() {
                    let label = label.clone();
                    panic!(
                        "{what}: {label} exited early ({status})\n{}",
                        self.outputs()
                    );
                }
            }
            if Instant::now() > end {
                panic!("{what}: not ready within {BOUND:?}\n{}", self.outputs());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

impl Drop for Helpers {
    fn drop(&mut self) {
        for (_, child) in &mut self.children {
            if let Ok(None) = child.try_wait() {
                let _ = child.kill();
            }
            let _ = child.wait();
        }
    }
}

fn ticket_count(dir: &Path) -> usize {
    fs::read_dir(dir).map_or(0, |entries| {
        entries
            .filter_map(Result::ok)
            .filter(|e| e.file_name().as_encoded_bytes().ends_with(b".json"))
            .count()
    })
}

#[test]
fn cross_process_fifo() {
    if std::env::var_os(HELPER_ENV).is_some() {
        run_queue_helper();
    }

    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("queue");
    let events = root.path().join("events");
    let go = root.path().join("go");
    let exe = std::env::current_exe().unwrap();
    let labels = ["A", "B", "C"];
    let mut helpers = Helpers {
        root: root.path().to_path_buf(),
        children: Vec::new(),
    };

    for (i, label) in labels.iter().enumerate() {
        let log = root.path().join(format!("{label}.out"));
        let out = fs::File::create(&log).unwrap();
        let mut cmd = Command::new(&exe);
        cmd.args([TEST_NAME, "--exact", "--nocapture", "--test-threads=1"])
            .env(HELPER_ENV, "1")
            .env("RIVAL_QUEUE_DIR", &dir)
            .env("RIVAL_QUEUE_LABEL", label)
            .env("RIVAL_QUEUE_EVENTS", &events)
            .env("RIVAL_QUEUE_GO", &go)
            // Task-owned homes: the helper never needs them, but nothing it
            // runs may reach the real ~/.rival.
            .env("HOME", root.path())
            .env("RIVAL_HOME", root.path().join("rival-home"))
            .stdin(Stdio::null())
            .stdout(out.try_clone().unwrap())
            .stderr(out);
        let child = crate::executor::process::spawn(&mut cmd)
            .unwrap_or_else(|e| panic!("start {label}: {e}"));
        drop(cmd);
        helpers.children.push((label.to_string(), child));
        // Enqueue order is deterministic: the next helper starts only once
        // this one's ticket is on disk.
        helpers.wait_until(&format!("{label} enqueued"), || ticket_count(&dir) == i + 1);
    }
    // A holds the slot until now; B and C are queued behind it in order.
    fs::write(&go, "").unwrap();

    let end = Instant::now() + BOUND;
    for i in 0..helpers.children.len() {
        loop {
            let (label, child) = &mut helpers.children[i];
            if let Some(status) = child.try_wait().unwrap() {
                let label = label.clone();
                assert!(
                    status.success(),
                    "proc {label} failed: {status}\noutput:\n{}",
                    helpers.output(&label)
                );
                break;
            }
            if Instant::now() > end {
                let label = label.clone();
                panic!(
                    "proc {label} still running after {BOUND:?}\noutput:\n{}",
                    helpers.output(&label)
                );
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    let got = read(&events);
    let want = "A ACQUIRED\nA RELEASED\nB ACQUIRED\nB RELEASED\nC ACQUIRED\nC RELEASED\n";
    assert_eq!(
        got,
        want,
        "FIFO or mutual exclusion violated\n{}",
        helpers.outputs()
    );
    assert_eq!(ticket_count(&dir), 0, "every helper released its ticket");
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

fn append(events: &Path, line: &str) {
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(events)
        .expect("open events");
    f.write_all(line.as_bytes()).expect("write event");
}

/// The helper child's body. Exits the process; never returns.
fn run_queue_helper() -> ! {
    let var = |k: &str| std::env::var_os(k).unwrap_or_else(|| panic!("{k} unset"));
    let dir = PathBuf::from(var("RIVAL_QUEUE_DIR"));
    let label = var("RIVAL_QUEUE_LABEL").into_string().unwrap();
    let events = PathBuf::from(var("RIVAL_QUEUE_EVENTS"));
    let go = PathBuf::from(var("RIVAL_QUEUE_GO"));

    let mut m = Manager::with_settings(dir, 1, Duration::from_millis(20), BOUND);
    m.session_live = Some(std::sync::Arc::new(|_: &str| false));
    if let Err(e) = m.enqueue("", &[], "test", "/tmp") {
        println!("{label} ENQUEUE_ERR {e:#}");
        std::process::exit(1);
    }
    if let Err(e) = m.wait_for_slot(&Context::background(), None) {
        println!("{label} WAIT_ERR {e}");
        std::process::exit(1);
    }
    append(&events, &format!("{label} ACQUIRED\n"));
    // Hold until every helper has enqueued, then a little longer so a
    // competitor that ignored the lock would show up between our lines.
    let end = Instant::now() + BOUND;
    while !go.exists() {
        if Instant::now() > end {
            println!("{label} TIMEOUT");
            std::process::exit(1);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    std::thread::sleep(Duration::from_millis(100));
    append(&events, &format!("{label} RELEASED\n"));
    m.release();
    std::process::exit(0);
}
