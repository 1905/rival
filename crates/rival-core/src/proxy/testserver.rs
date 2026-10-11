//! A canned HTTP server on `127.0.0.1:0` for tests: every request gets the
//! same status and body. No real proxy and no network.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

/// What the fake saw: the request line and the Authorization header.
pub(crate) type Seen = Arc<Mutex<Vec<(String, String)>>>;

/// Serves every connection with `status` and `body` until the test ends.
pub(crate) fn serve(status: u16, body: &'static str) -> (String, Seen) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let seen: Seen = Arc::default();
    let log = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let mut auth = String::new();
            loop {
                let mut h = String::new();
                if reader.read_line(&mut h).unwrap() == 0 || h == "\r\n" {
                    break;
                }
                if let Some((name, value)) = h.split_once(':')
                    && name.eq_ignore_ascii_case("authorization")
                {
                    auth = value.trim().to_string();
                }
            }
            log.lock().unwrap().push((line.trim().to_string(), auth));
            let _ = write!(
                stream,
                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    (url, seen)
}
