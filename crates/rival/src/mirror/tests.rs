use std::sync::mpsc;
use std::time::Instant;

use super::*;

/// Collects what the stdout thread writes.
#[derive(Clone, Default)]
struct Collect(Arc<Mutex<Vec<u8>>>);

impl Write for Collect {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A stdout whose reader never reads: every write blocks until the test
/// ends (the sender is dropped).
struct Stalled(Mutex<mpsc::Receiver<()>>);

impl Write for Stalled {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        let _ = self.0.lock().unwrap().recv();
        Err(io::ErrorKind::BrokenPipe.into())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A stdout whose reader closed the pipe.
struct Closed;

impl Write for Closed {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::new(io::ErrorKind::BrokenPipe, "broken pipe"))
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn a_live_run_writes_every_line_in_order() {
    let out = Collect::default();
    let mirror = LiveMirror::spawn(Box::new(out.clone())).unwrap();
    let (ctx, _cancel) = Context::background().with_cancel();
    let mut sink = mirror.sink(&ctx);
    let mut want = Vec::new();
    for i in 0..2000 {
        let line = format!("line {i}\n");
        sink.write_all(line.as_bytes()).unwrap();
        want.extend_from_slice(line.as_bytes());
    }
    mirror.finish(&ctx);
    assert_eq!(*out.0.lock().unwrap(), want);
}

/// The reader holds the pipe open but stops reading. A full queue must not
/// hold the run past its context, and finishing must not wait forever.
#[test]
fn a_stalled_reader_does_not_outlive_the_context() {
    let (_unblock, rx) = mpsc::channel();
    let mirror = LiveMirror::spawn(Box::new(Stalled(Mutex::new(rx)))).unwrap();
    let (ctx, cancel) = Context::background().with_cancel();
    let mut sink = mirror.sink(&ctx);
    // Fill the queue past its limit while the context is live.
    let chunk = vec![b'x'; 64 * 1024];
    let filler = std::thread::scope(|s| {
        let h = s.spawn(|| {
            for _ in 0..(2 * MAX_QUEUED / chunk.len() + 2) {
                sink.write_all(&chunk).unwrap();
            }
        });
        std::thread::sleep(Duration::from_millis(200));
        let blocked = !h.is_finished();
        cancel.cancel();
        let started = Instant::now();
        h.join().unwrap();
        (blocked, started.elapsed())
    });
    assert!(filler.0, "a full queue waits while the run is live");
    assert!(filler.1 < Duration::from_secs(2), "{:?}", filler.1);
    let started = Instant::now();
    mirror.finish(&ctx);
    let took = started.elapsed();
    assert!(took >= PIPE_DRAIN_GRACE - POLL, "{took:?}");
    assert!(took < PIPE_DRAIN_GRACE + Duration::from_secs(2), "{took:?}");
}

#[test]
fn a_closed_reader_reports_its_error_on_later_lines() {
    let mirror = LiveMirror::spawn(Box::new(Closed)).unwrap();
    let (ctx, _cancel) = Context::background().with_cancel();
    let mut sink = mirror.sink(&ctx);
    sink.write_all(b"first\n").unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let err = loop {
        if let Err(e) = sink.write(b"next\n") {
            break e;
        }
        assert!(Instant::now() < deadline, "the write error never surfaced");
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(err.kind(), io::ErrorKind::BrokenPipe);
    mirror.finish(&ctx);
}
