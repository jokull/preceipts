//! What the code just served, and what it answered.
//!
//! The TLS proxy is already in the path of every request into every service,
//! so recording them costs the app under test nothing: no middleware, no
//! instrumentation, no library to add and remember to remove. That is the
//! whole reason this instrument belongs here rather than in a framework
//! plugin — it works on a Rails app, a Go binary, and a Vite dev server
//! identically, because it never enters any of them.
//!
//! **Headers, not bodies.** A body can be a video, a 40MB JSON export, or a
//! password, and a ring buffer that holds one is a ring buffer that holds a
//! secret. Sizes are recorded; contents are not. What an agent almost always
//! needs — did my code get the request it expected, what status did it
//! answer, how long did it take, what content type came back — lives entirely
//! in the head of each message.
//!
//! **Sniffed, never parsed.** The proxy copies bytes in both directions and
//! must keep doing exactly that: a transcript that reassembles requests would
//! become a proxy that can get HTTP wrong, and getting HTTP wrong in the path
//! of someone's dev server is a much worse bug than a missing log line. So we
//! read the first line and the headers of each message as they stream past,
//! and if a message does not look like HTTP we record nothing and keep
//! copying.

use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

/// How many exchanges a workspace keeps. Roughly a browser session's worth of
/// traffic; enough to answer "what did that page load do" long after it did.
pub const CAPACITY: usize = 2000;

/// The largest head we will buffer while looking for the end of the headers.
///
/// A message whose headers exceed this is not recorded, and is still proxied
/// untouched. Bounded because the alternative is letting a peer decide how
/// much memory to take by never sending a blank line.
const MAX_HEAD: usize = 32 * 1024;

pub use preceipts_proto::Exchange;

/// A workspace's recent HTTP, newest last.
#[derive(Debug, Default)]
pub struct Transcript {
    entries: Mutex<VecDeque<Exchange>>,
    next_id: std::sync::atomic::AtomicU64,
}

impl Transcript {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Record a request. Returns its id, so the response can complete it.
    pub fn begin(&self, host: &str, method: &str, path: &str, request_bytes: u64) -> u64 {
        let id = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut entries = self.entries.lock();
        if entries.len() == CAPACITY {
            entries.pop_front();
        }
        entries.push_back(Exchange {
            id,
            host: host.to_string(),
            method: method.to_string(),
            path: path.to_string(),
            status: None,
            duration_ms: None,
            request_bytes,
            response_bytes: 0,
            content_type: None,
            at: now_secs(),
        });
        id
    }

    /// Complete the exchange with this id.
    ///
    /// Searched from the back, because a response almost always belongs to a
    /// recent request. An id the ring has already evicted simply is not found,
    /// which is the correct outcome — the exchange is gone, and nothing else
    /// should inherit its status.
    pub fn complete(
        &self,
        id: u64,
        status: u16,
        response_bytes: u64,
        content_type: Option<String>,
        duration: Duration,
    ) {
        let mut entries = self.entries.lock();
        let Some(entry) = entries.iter_mut().rev().find(|e| e.id == id) else {
            return;
        };
        entry.status = Some(status);
        entry.response_bytes = response_bytes;
        entry.content_type = content_type;
        entry.duration_ms = Some(duration.as_millis() as u64);
    }

    pub fn len(&self) -> usize {
        self.entries.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.lock().is_empty()
    }

    /// Recent exchanges, newest last, optionally filtered.
    pub fn recent(&self, host: Option<&str>, since_secs: Option<i64>) -> Vec<Exchange> {
        let cutoff = since_secs.map(|s| now_secs() - s);
        self.entries
            .lock()
            .iter()
            .filter(|e| host.is_none_or(|h| e.host == h))
            .filter(|e| cutoff.is_none_or(|c| e.at >= c))
            .cloned()
            .collect()
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// The request line and headers of an HTTP message, if this looks like one.
pub struct Head {
    pub start_line: String,
    pub headers: Vec<(String, String)>,
    /// Bytes consumed by the head, including the blank line.
    pub len: usize,
}

impl Head {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// Parse a message head out of `buffer`, if a complete one is there.
///
/// `None` means "not yet, or not HTTP" — both are handled the same way by the
/// caller, which is to keep copying bytes and stop trying.
pub fn parse_head(buffer: &[u8]) -> Option<Head> {
    if buffer.len() > MAX_HEAD {
        return None;
    }
    let end = find_blank_line(buffer)?;
    let text = std::str::from_utf8(&buffer[..end]).ok()?;
    // Split on LF and trim any CR: a well-behaved peer sends CRLF, and a
    // hand-rolled client or a test fixture often does not. Being strict here
    // would mean recording nothing for traffic the backend handles fine.
    let mut lines = text
        .split('\n')
        .map(|line| line.trim_end_matches('\r'))
        .filter(|line| !line.is_empty());
    let start_line = lines.next()?.to_string();
    // Anything without three space-separated parts is not a request or status
    // line, which is the cheapest way to notice we are proxying something
    // that is not HTTP at all.
    if start_line.split(' ').count() < 3 {
        return None;
    }
    let headers = lines
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            Some((name.trim().to_string(), value.trim().to_string()))
        })
        .collect();
    Some(Head {
        start_line,
        headers,
        len: end,
    })
}

/// Index just past the CRLFCRLF (or LFLF) that ends a message head.
fn find_blank_line(buffer: &[u8]) -> Option<usize> {
    buffer
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| i + 4)
        .or_else(|| buffer.windows(2).position(|w| w == b"\n\n").map(|i| i + 2))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_head_yields_its_method_and_path() {
        let raw = b"GET /health?deep=1 HTTP/1.1\r\nHost: api.localhost\r\nAccept: */*\r\n\r\nbody";
        let head = parse_head(raw).expect("a request head");
        assert_eq!(head.start_line, "GET /health?deep=1 HTTP/1.1");
        assert_eq!(head.header("host"), Some("api.localhost"));
        assert_eq!(
            head.header("HOST"),
            Some("api.localhost"),
            "case-insensitive"
        );
        assert_eq!(&raw[head.len..], b"body", "the body is left where it was");
    }

    #[test]
    fn a_response_head_yields_its_status_and_type() {
        let raw = b"HTTP/1.1 404 Not Found\r\nContent-Type: text/html; charset=utf-8\r\n\r\n";
        let head = parse_head(raw).expect("a response head");
        assert_eq!(head.start_line, "HTTP/1.1 404 Not Found");
        assert_eq!(
            head.header("content-type"),
            Some("text/html; charset=utf-8")
        );
    }

    /// The proxy must keep working for anything that is not HTTP — a
    /// websocket after upgrade, gRPC, a raw TCP protocol someone tunnelled.
    /// Recording nothing is the correct outcome; refusing to proxy is not.
    #[test]
    fn something_that_is_not_http_records_nothing() {
        assert!(parse_head(b"\x16\x03\x01\x00\xa5binary\r\n\r\n").is_none());
        assert!(parse_head(b"PING\r\n\r\n").is_none(), "too few parts");
    }

    #[test]
    fn an_incomplete_head_is_not_yet_a_head() {
        assert!(parse_head(b"GET / HTTP/1.1\r\nHost: x").is_none());
    }

    /// A peer that never sends a blank line must not be able to make us
    /// buffer without limit.
    #[test]
    fn an_unbounded_head_is_refused_rather_than_buffered() {
        let huge = vec![b'a'; MAX_HEAD + 1];
        assert!(parse_head(&huge).is_none());
    }

    #[test]
    fn lf_only_heads_still_parse() {
        let head = parse_head(b"GET / HTTP/1.1\nHost: x\n\n").expect("lenient about CR");
        assert_eq!(head.header("host"), Some("x"));
    }

    #[test]
    fn an_exchange_records_both_halves() {
        let transcript = Transcript::new();
        let id = transcript.begin("api.localhost", "GET", "/health", 120);
        transcript.complete(
            id,
            200,
            48,
            Some("application/json".to_string()),
            Duration::from_millis(12),
        );
        let recent = transcript.recent(None, None);
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].status, Some(200));
        assert_eq!(recent[0].duration_ms, Some(12));
        assert_eq!(recent[0].content_type.as_deref(), Some("application/json"));
    }

    /// A request with no response is what a hang looks like, and it should be
    /// visible as one rather than missing.
    #[test]
    fn a_request_with_no_response_is_still_recorded() {
        let transcript = Transcript::new();
        transcript.begin("api.localhost", "POST", "/slow", 10);
        let recent = transcript.recent(None, None);
        assert_eq!(recent[0].status, None);
        assert_eq!(recent[0].duration_ms, None);
    }

    #[test]
    fn the_ring_forgets_the_oldest_first() {
        let transcript = Transcript::new();
        for i in 0..CAPACITY + 10 {
            transcript.begin("api.localhost", "GET", &format!("/{i}"), 0);
        }
        let recent = transcript.recent(None, None);
        assert_eq!(recent.len(), CAPACITY);
        assert_eq!(recent[0].path, "/10", "the first ten are gone");
        assert_eq!(recent[CAPACITY - 1].path, format!("/{}", CAPACITY + 9));
    }

    /// The hazard eviction creates: a response arriving after its request has
    /// been pushed out must not stamp its status onto an unrelated exchange.
    #[test]
    fn a_response_to_an_evicted_request_is_dropped_not_misattributed() {
        let transcript = Transcript::new();
        let id = transcript.begin("api.localhost", "GET", "/first", 0);
        for i in 0..CAPACITY + 5 {
            transcript.begin("api.localhost", "GET", &format!("/later-{i}"), 0);
        }
        transcript.complete(id, 500, 0, None, Duration::from_millis(1));
        assert!(
            transcript
                .recent(None, None)
                .iter()
                .all(|e| e.status.is_none()),
            "nobody inherited the evicted request's status"
        );
    }

    #[test]
    fn filtering_by_host_and_age_narrows_the_answer() {
        let transcript = Transcript::new();
        transcript.begin("api.localhost", "GET", "/a", 0);
        transcript.begin("web.localhost", "GET", "/b", 0);
        assert_eq!(transcript.recent(Some("api.localhost"), None).len(), 1);
        assert_eq!(transcript.recent(None, Some(60)).len(), 2);
        assert_eq!(
            transcript.recent(None, Some(-1)).len(),
            0,
            "a cutoff in the future matches nothing"
        );
    }
}
