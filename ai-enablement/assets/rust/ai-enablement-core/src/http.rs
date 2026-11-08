//! The HTTP client of the adapters (feature `http`): [`UreqClient`], an [`HttpClient`] over ureq 3 (blocking; call
//! it from a worker thread). With the feature `tls` it speaks https through rustls, trusting Mozilla's roots (webpki);
//! with `tls-platform`, the operating system's certificate store instead, which is what works behind an antivirus or
//! a company proxy that inspects TLS with a root of its own. Without either, only plain http (local servers: LM
//! Studio, Ollama), and an https address fails as a network error.
//!
//! Timeouts. The JS transport has one idle timer: no response, or no new bytes, for `timeoutMs` fails the request.
//! Here `HttpRequest::timeout_ms` bounds:
//! - each phase before the response separately: name resolution, connecting (including the TLS handshake), sending
//!   the request, waiting for the response headers. So the worst case to the response is a few times `timeout_ms`,
//!   not `timeout_ms`: ureq has no single "time to the response" budget that stops at the headers;
//! - each read of the body (ureq's per-read timeout): a stream that goes silent for `timeout_ms` fails, a long answer
//!   that keeps flowing is never cut off. That is the JS idle timer.
//!
//! What it cannot do: a per-read timeout restarts on every transport read, so a server trickling a byte at a time
//! keeps a stream alive (as in JS); and time while the caller is not reading (the read-ahead below is full) is not
//! counted.
//!
//! Cancellation. The request runs on a worker thread of its own, which hands the body over in chunks through a
//! bounded channel (at most 8 reads ahead). The caller's thread waits for them in 50 ms steps and checks the
//! [`CancelToken`] in between, so a cancel takes effect within 50 ms whether the request is waiting for its response
//! or for the next bytes. A cancel, or dropping a response before its end, also shuts the request's ureq agent down,
//! which closes the connection so the server stops generating. Where the platform does not wake a blocked socket read
//! on that (Windows), the worker ends at its next read (bytes or the read timeout); nothing waits for it. One agent
//! per request means no connection reuse between requests: a connection setup per model call, small next to the call.

use crate::transport::{CancelToken, HttpClient, HttpFailure, HttpRequest, HttpResponse};
use std::io::{self, Read};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::thread;
use std::time::Duration;
use ureq::Agent;

/// How often a waiting caller looks at the cancel token.
const CANCEL_POLL: Duration = Duration::from_millis(50);
/// Body reads the worker thread may be ahead of the caller.
const READ_AHEAD: usize = 8;

/// An [`HttpClient`] over ureq 3. Non-2xx responses come back as responses (the transport classifies them).
#[derive(Debug, Clone)]
pub struct UreqClient {
    user_agent: Option<String>,
}

impl Default for UreqClient {
    fn default() -> Self {
        Self::new()
    }
}

impl UreqClient {
    pub fn new() -> Self {
        UreqClient { user_agent: None }
    }

    /// Send this `User-Agent` (default: ureq's own).
    pub fn with_user_agent(mut self, user_agent: impl Into<String>) -> Self {
        self.user_agent = Some(user_agent.into());
        self
    }

    fn agent(&self, timeout_ms: u64) -> Agent {
        let t = Some(Duration::from_millis(timeout_ms.max(1)));
        let mut config = Agent::config_builder()
            .http_status_as_error(false)
            .timeout_resolve(t)
            .timeout_connect(t)
            .timeout_send_request(t)
            .timeout_send_body(t)
            .timeout_recv_response(t)
            .timeout_per_read(t)
            .timeout_per_write(t);
        if let Some(ua) = &self.user_agent {
            config = config.user_agent(ua.as_str());
        }
        #[cfg(feature = "tls-platform")]
        {
            use ureq::tls::{RootCerts, TlsConfig};
            config = config.tls_config(TlsConfig::builder().root_certs(RootCerts::PlatformVerifier).build());
        }
        config.build().into()
    }
}

/// What the worker thread hands over: the response head once, then the body.
enum Msg {
    Head(Result<(u16, String), HttpFailure>),
    Chunk(Vec<u8>),
    End,
    Fail(io::Error),
}

fn is_timeout(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::TimedOut
        || e.get_ref()
            .and_then(|x| x.downcast_ref::<ureq::Error>())
            .is_some_and(|u| matches!(u, ureq::Error::Timeout(_)))
}

fn failure(e: ureq::Error, cancel: &CancelToken) -> HttpFailure {
    if cancel.is_cancelled() {
        return HttpFailure::Cancelled;
    }
    match e {
        ureq::Error::Timeout(_) => HttpFailure::Timeout,
        ureq::Error::Io(io) if io.kind() == io::ErrorKind::TimedOut => HttpFailure::Timeout,
        other => HttpFailure::Network(other.to_string()),
    }
}

/// The worker: send, hand over the head, then the body until its end, an error, or the caller lets go.
fn pump<B: ureq::AsSendBody>(agent: Agent, request: ureq::http::Request<B>, cancel: CancelToken, tx: SyncSender<Msg>) {
    let response = match agent.run(request) {
        Ok(r) => r,
        Err(e) => {
            let _ = tx.send(Msg::Head(Err(failure(e, &cancel))));
            return;
        }
    };
    let status = response.status().as_u16();
    let content_type =
        response.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
    if tx.send(Msg::Head(Ok((status, content_type)))).is_err() {
        return;
    }
    let mut reader = response.into_body().into_reader();
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        let msg = match reader.read(&mut buf) {
            Ok(0) => Msg::End,
            Ok(n) => Msg::Chunk(buf[..n].to_vec()),
            Err(e) if is_timeout(&e) => Msg::Fail(io::Error::new(io::ErrorKind::TimedOut, e)),
            Err(e) => Msg::Fail(e),
        };
        let last = !matches!(msg, Msg::Chunk(_));
        if tx.send(msg).is_err() || last {
            return;
        }
    }
}

/// Wait for the worker's next message, checking the token every [`CANCEL_POLL`]. A cancel shuts the agent down.
fn next(rx: &Receiver<Msg>, cancel: &CancelToken, agent: &Agent) -> Result<Msg, HttpFailure> {
    loop {
        if cancel.is_cancelled() {
            agent.clone().shutdown();
            return Err(HttpFailure::Cancelled);
        }
        match rx.recv_timeout(CANCEL_POLL) {
            Ok(m) => return Ok(m),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Err(HttpFailure::Network("the request stopped".into())),
        }
    }
}

/// The response body, as the worker hands it over. A cancel is an error of kind `Other` ("cancelled"); the
/// transport reports it as cancelled because the token is set.
struct Body {
    rx: Receiver<Msg>,
    chunk: Vec<u8>,
    at: usize,
    ended: bool,
    cancel: CancelToken,
    agent: Agent,
}

impl Body {
    fn stop(&mut self, e: io::Error) -> io::Result<usize> {
        self.ended = true;
        Err(e)
    }
}

impl Read for Body {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.cancel.is_cancelled() {
            self.agent.clone().shutdown();
            return self.stop(io::Error::other("cancelled"));
        }
        if self.at >= self.chunk.len() {
            if self.ended {
                return Ok(0);
            }
            match next(&self.rx, &self.cancel, &self.agent) {
                Ok(Msg::Chunk(c)) => {
                    self.chunk = c;
                    self.at = 0;
                }
                Ok(Msg::Fail(e)) => return self.stop(e),
                Ok(Msg::End | Msg::Head(_)) => {
                    self.ended = true;
                    return Ok(0);
                }
                Err(HttpFailure::Cancelled) => return self.stop(io::Error::other("cancelled")),
                Err(HttpFailure::Timeout) => return self.stop(io::ErrorKind::TimedOut.into()),
                Err(HttpFailure::Network(m)) => return self.stop(io::Error::other(m)),
            }
        }
        let n = buf.len().min(self.chunk.len() - self.at);
        buf[..n].copy_from_slice(&self.chunk[self.at..self.at + n]);
        self.at += n;
        Ok(n)
    }
}

impl Drop for Body {
    fn drop(&mut self) {
        // Let go before the end: close the connection, so the worker (and the server) stop.
        if !self.ended {
            self.agent.clone().shutdown();
        }
    }
}

impl HttpClient for UreqClient {
    fn send(&self, request: &HttpRequest, cancel: &CancelToken) -> Result<HttpResponse, HttpFailure> {
        if cancel.is_cancelled() {
            return Err(HttpFailure::Cancelled);
        }
        let agent = self.agent(request.timeout_ms);
        let mut builder = ureq::http::Request::builder().method(request.method.as_str()).uri(request.url.as_str());
        for (k, v) in &request.headers {
            builder = builder.header(k.as_str(), v.as_str());
        }
        let invalid = |e: ureq::http::Error| HttpFailure::Network(e.to_string());
        let (tx, rx) = mpsc::sync_channel(READ_AHEAD);
        let (a, c) = (agent.clone(), cancel.clone());
        let worker = thread::Builder::new().name("ai-http".into());
        let spawned = match &request.body {
            Some(body) => {
                let r = builder.body(body.clone().into_bytes()).map_err(invalid)?;
                worker.spawn(move || pump(a, r, c, tx))
            }
            None => {
                let r = builder.body(()).map_err(invalid)?;
                worker.spawn(move || pump(a, r, c, tx))
            }
        };
        spawned.map_err(|e| HttpFailure::Network(format!("could not start the request thread: {e}")))?;
        let (status, content_type) = match next(&rx, cancel, &agent)? {
            Msg::Head(head) => head?,
            _ => return Err(HttpFailure::Network("no response".into())),
        };
        let body = Body { rx, chunk: Vec::new(), at: 0, ended: false, cancel: cancel.clone(), agent };
        Ok(HttpResponse { status, content_type, body: Box::new(body) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;
    use std::sync::{Mutex, MutexGuard};
    use std::time::Instant;

    /// The socket tests run one at a time: on some Windows machines (a traffic-filter driver), the first bytes of
    /// several new loopback connections opened at once are held for about 10 s, which breaks the timing checks.
    fn serial() -> MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A one-connection HTTP server: reads the request head, then plays `script` (a pause, then bytes to write).
    fn serve(script: Vec<(Duration, &'static str)>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = [0u8; 4096];
            let mut seen = Vec::new();
            while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = s.read(&mut buf).unwrap_or(0);
                if n == 0 {
                    return;
                }
                seen.extend_from_slice(&buf[..n]);
            }
            for (pause, bytes) in script {
                thread::sleep(pause);
                if s.write_all(bytes.as_bytes()).is_err() {
                    return;
                }
                let _ = s.flush();
            }
            thread::sleep(Duration::from_millis(300));
        });
        format!("http://{addr}/v1/chat")
    }

    fn get(url: &str, timeout_ms: u64) -> HttpRequest {
        HttpRequest { method: "GET".into(), url: url.into(), headers: vec![], body: None, timeout_ms }
    }

    const NOW: Duration = Duration::ZERO;
    const STREAM_HEAD: &str =
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n";

    fn cancel_after(ms: u64) -> CancelToken {
        let cancel = CancelToken::new();
        let c = cancel.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(ms));
            c.cancel();
        });
        cancel
    }

    #[test]
    fn error_statuses_are_responses() {
        let _serial = serial();
        let url =
            serve(vec![(NOW, "HTTP/1.1 404 Not Found\r\nContent-Type: text/plain\r\nContent-Length: 4\r\n\r\nnope")]);
        let mut r = UreqClient::new().send(&get(&url, 5000), &CancelToken::new()).unwrap();
        let mut text = String::new();
        r.body.read_to_string(&mut text).unwrap();
        assert_eq!((r.status, r.content_type.as_str(), text.as_str()), (404, "text/plain", "nope"));
    }

    #[test]
    fn a_stream_arrives_in_pieces_until_its_end() {
        let _serial = serial();
        let url = serve(vec![
            (NOW, STREAM_HEAD),
            (NOW, "6\r\ndata: \r\n"),
            (Duration::from_millis(100), "4\r\nmore\r\n0\r\n\r\n"),
        ]);
        let mut r = UreqClient::new().send(&get(&url, 5000), &CancelToken::new()).unwrap();
        let mut text = String::new();
        r.body.read_to_string(&mut text).unwrap();
        assert_eq!((r.content_type.as_str(), text.as_str()), ("text/event-stream", "data: more"));
    }

    #[test]
    fn a_silent_stream_times_out() {
        let _serial = serial();
        let url = serve(vec![(NOW, STREAM_HEAD), (NOW, "6\r\ndata: \r\n"), (Duration::from_millis(1500), "0\r\n\r\n")]);
        let mut r = UreqClient::new().send(&get(&url, 300), &CancelToken::new()).unwrap();
        let mut buf = [0u8; 64];
        assert_eq!(r.body.read(&mut buf).unwrap(), 6);
        let started = Instant::now();
        let e = r.body.read(&mut buf).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_millis(1400));
    }

    #[test]
    fn no_response_times_out() {
        let _serial = serial();
        let url = serve(vec![(Duration::from_millis(1500), "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")]);
        let started = Instant::now();
        let e = UreqClient::new().send(&get(&url, 300), &CancelToken::new()).err().unwrap();
        assert_eq!(e, HttpFailure::Timeout);
        assert!(started.elapsed() < Duration::from_millis(1400), "took {:?}", started.elapsed());
    }

    #[test]
    fn cancel_interrupts_the_wait_for_a_response() {
        let _serial = serial();
        let url = serve(vec![(Duration::from_millis(3000), "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")]);
        let cancel = cancel_after(200);
        let started = Instant::now();
        let e = UreqClient::new().send(&get(&url, 10_000), &cancel).err().unwrap();
        assert_eq!(e, HttpFailure::Cancelled);
        assert!(started.elapsed() < Duration::from_millis(1000), "took {:?}", started.elapsed());
    }

    #[test]
    fn cancel_interrupts_a_body_read() {
        let _serial = serial();
        let url = serve(vec![(NOW, STREAM_HEAD), (NOW, "6\r\ndata: \r\n"), (Duration::from_millis(3000), "0\r\n\r\n")]);
        let cancel = CancelToken::new();
        let mut r = UreqClient::new().send(&get(&url, 10_000), &cancel).unwrap();
        let mut buf = [0u8; 64];
        assert_eq!(r.body.read(&mut buf).unwrap(), 6);
        let c = cancel.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            c.cancel();
        });
        let started = Instant::now();
        let e = r.body.read(&mut buf).unwrap_err();
        assert_eq!(e.to_string(), "cancelled");
        assert!(started.elapsed() < Duration::from_millis(1000), "took {:?}", started.elapsed());
    }

    #[test]
    fn cancelled_before_sending() {
        let _serial = serial();
        let cancel = CancelToken::new();
        cancel.cancel();
        let e = UreqClient::new().send(&get("http://127.0.0.1:9/", 1000), &cancel).err().unwrap();
        assert_eq!(e, HttpFailure::Cancelled);
    }

    #[test]
    fn unreachable_is_a_network_failure() {
        let _serial = serial();
        // Bind and drop: nothing listens on that port any more (Windows takes about 2 s to refuse).
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let url = format!("http://127.0.0.1:{port}/");
        let e = UreqClient::new().send(&get(&url, 10_000), &CancelToken::new()).err().unwrap();
        assert!(matches!(e, HttpFailure::Network(_)), "{e:?}");
    }

    #[test]
    fn posts_the_body_with_its_headers() {
        let _serial = serial();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/x", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut seen = Vec::new();
            let mut buf = [0u8; 4096];
            while !String::from_utf8_lossy(&seen).ends_with("{\"a\":1}") {
                let n = s.read(&mut buf).unwrap();
                if n == 0 {
                    break;
                }
                seen.extend_from_slice(&buf[..n]);
            }
            s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}").unwrap();
            String::from_utf8_lossy(&seen).to_lowercase()
        });
        let req = HttpRequest {
            method: "POST".into(),
            url,
            headers: vec![
                ("Content-Type".into(), "application/json".into()),
                ("X-Requested-With".into(), "ai-agent-drawer".into()),
            ],
            body: Some("{\"a\":1}".into()),
            timeout_ms: 5000,
        };
        let mut r = UreqClient::new().send(&req, &CancelToken::new()).unwrap();
        let mut text = String::new();
        r.body.read_to_string(&mut text).unwrap();
        let seen = server.join().unwrap();
        assert!(seen.starts_with("post /x http/1.1\r\n"), "{seen}");
        assert!(seen.contains("x-requested-with: ai-agent-drawer\r\n"));
        assert!(seen.contains("content-length: 7\r\n"));
        assert_eq!(text, "{}");
    }
}
