//! A non-blocking binary message socket to the collaboration server that reconnects by itself.
//!
//! [`connect`] opens the platform's WebSocket: a background thread with a blocking `tungstenite`
//! client on desktop (TLS through rustls for `wss://`), the browser's `WebSocket` on the web. Both
//! hand their events to the UI through [`Socket::poll`], once per frame, and ask egui for a
//! repaint when one arrives. A dropped connection is retried with a growing delay; after each
//! [`Event::Opened`] the caller says hello again (`Collab::on_connect`): messages sent while the
//! socket was down are dropped, the sync protocol catches up on reconnect.

/// What happened on the socket since the last poll.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The connection is (again) open.
    Opened,
    /// A binary message from the server.
    Message(Vec<u8>),
    /// The connection closed (or couldn't open); it is retried.
    Closed(String),
    /// Something went wrong (a close follows).
    Error(String),
}

/// A message socket: queued sends, polled events.
pub trait Socket {
    /// Queue a binary message (dropped while the connection is down).
    fn send(&mut self, msg: Vec<u8>);
    /// Everything that happened since the last poll.
    fn poll(&mut self) -> Vec<Event>;
}

/// The first wait before reconnecting, and the longest (ms).
pub const BACKOFF_MIN_MS: u64 = 500;
pub const BACKOFF_MAX_MS: u64 = 10_000;

/// The wait before reconnect attempt `attempt` (0 = the first retry): doubles up to the maximum.
pub fn backoff_ms(attempt: u32) -> u64 {
    BACKOFF_MIN_MS.saturating_mul(1u64 << attempt.min(16)).min(BACKOFF_MAX_MS)
}

/// Milliseconds since the Unix epoch (the collaboration protocol's clock).
pub fn clock_ms() -> u64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
    }
    #[cfg(target_arch = "wasm32")]
    {
        // Saturating cast of a finite, positive number of milliseconds.
        js_sys::Date::now().max(0.0) as u64
    }
}

/// A random number (room ids, client ids): the OS's randomness on desktop, the browser's on the
/// web.
pub fn random_u64() -> u64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::hash::{BuildHasher, Hasher};
        // `RandomState` is seeded from the OS's random source.
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(clock_ms());
        h.finish()
    }
    #[cfg(target_arch = "wasm32")]
    {
        let hi = (js_sys::Math::random() * 4294967296.0) as u64;
        let lo = (js_sys::Math::random() * 4294967296.0) as u64;
        (hi << 32) ^ lo ^ clock_ms()
    }
}

/// Open the platform's socket to `url` (`ws://` or `wss://`); `repaint` is asked for a frame when
/// an event arrives.
pub fn connect(url: &str, repaint: Option<egui::Context>) -> Box<dyn Socket> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        Box::new(native::NativeSocket::open(url, repaint))
    }
    #[cfg(target_arch = "wasm32")]
    {
        Box::new(web::WebSocket::open(url, repaint))
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::net::{TcpStream, ToSocketAddrs};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
    use std::time::Duration;

    use tungstenite::client::IntoClientRequest;
    use tungstenite::stream::MaybeTlsStream;
    use tungstenite::{Message, WebSocket};

    use super::{Event, Socket, backoff_ms};

    /// How long a read waits before the thread looks at what there is to send: the most a sent
    /// message waits.
    const POLL: Duration = Duration::from_millis(10);
    /// Connecting and the WebSocket handshake give up after this long.
    const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);

    /// The socket's thread: connects, reads and writes, reconnects.
    pub struct NativeSocket {
        out: Sender<Vec<u8>>,
        events: Receiver<Event>,
        stop: Arc<AtomicBool>,
    }

    impl NativeSocket {
        pub fn open(url: &str, repaint: Option<egui::Context>) -> Self {
            let (out, out_rx) = channel::<Vec<u8>>();
            let (ev_tx, events) = channel::<Event>();
            let stop = Arc::new(AtomicBool::new(false));
            let worker = Worker { url: url.to_string(), out: out_rx, events: ev_tx, stop: stop.clone(), repaint };
            let spawned = std::thread::Builder::new().name("vectorcraft-collab".into()).spawn(move || worker.run());
            if let Err(e) = spawned {
                // No thread, no socket: say so through the channel the UI polls.
                let (tx, rx) = channel();
                let _ = tx.send(Event::Error(format!("couldn't start the connection: {e}")));
                return Self { out, events: rx, stop };
            }
            Self { out, events, stop }
        }
    }

    impl Socket for NativeSocket {
        fn send(&mut self, msg: Vec<u8>) {
            // The thread is gone only when the socket is being dropped: nothing to deliver to.
            let _ = self.out.send(msg);
        }

        fn poll(&mut self) -> Vec<Event> {
            self.events.try_iter().collect()
        }
    }

    impl Drop for NativeSocket {
        fn drop(&mut self) {
            // The thread sends what is queued (the goodbye), closes the socket and ends.
            self.stop.store(true, Ordering::SeqCst);
        }
    }

    struct Worker {
        url: String,
        out: Receiver<Vec<u8>>,
        events: Sender<Event>,
        stop: Arc<AtomicBool>,
        repaint: Option<egui::Context>,
    }

    type Ws = WebSocket<MaybeTlsStream<TcpStream>>;

    impl Worker {
        fn emit(&self, e: Event) {
            let _ = self.events.send(e);
            if let Some(ctx) = &self.repaint {
                ctx.request_repaint();
            }
        }

        fn stopped(&self) -> bool {
            self.stop.load(Ordering::SeqCst)
        }

        fn run(self) {
            let mut attempt = 0u32;
            while !self.stopped() {
                match open(&self.url) {
                    Ok(mut ws) => {
                        // Whatever was queued while down belongs to the old connection.
                        while self.out.try_recv().is_ok() {}
                        attempt = 0;
                        self.emit(Event::Opened);
                        let reason = self.serve(&mut ws);
                        let _ = ws.close(None);
                        let _ = ws.flush();
                        if self.stopped() {
                            return;
                        }
                        self.emit(Event::Closed(reason));
                    }
                    Err(e) => {
                        if self.stopped() {
                            return;
                        }
                        self.emit(Event::Error(e.clone()));
                        self.emit(Event::Closed(e));
                    }
                }
                // Wait before retrying, waking up to stop.
                let mut left = backoff_ms(attempt);
                attempt = attempt.saturating_add(1);
                while left > 0 && !self.stopped() {
                    let step = left.min(100);
                    std::thread::sleep(Duration::from_millis(step));
                    left -= step;
                }
            }
        }

        /// Read and write until the connection ends (→ why) or the socket is dropped.
        fn serve(&self, ws: &mut Ws) -> String {
            loop {
                match ws.read() {
                    Ok(Message::Binary(b)) => self.emit(Event::Message(b.to_vec())),
                    Ok(Message::Close(frame)) => {
                        return frame.map_or_else(|| "the server closed the connection".into(), |f| format!("closed: {} {}", f.code, f.reason));
                    }
                    // Pings are answered by tungstenite; text isn't part of the protocol.
                    Ok(_) => {}
                    Err(tungstenite::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                    Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => return "connection closed".into(),
                    Err(e) => return e.to_string(),
                }
                loop {
                    match self.out.try_recv() {
                        Ok(msg) => {
                            if let Err(e) = ws.send(Message::Binary(msg.into())) {
                                return e.to_string();
                            }
                        }
                        Err(TryRecvError::Empty) => break,
                        // The UI dropped the socket after queueing its last messages.
                        Err(TryRecvError::Disconnected) => return "left".into(),
                    }
                }
                if self.stopped() {
                    return "left".into();
                }
            }
        }
    }

    /// Connect (with a timeout) and do the WebSocket handshake, TLS for `wss://`.
    fn open(url: &str) -> Result<Ws, String> {
        let request = url.into_client_request().map_err(|e| format!("{url}: {e}"))?;
        let uri = request.uri();
        let host = uri.host().ok_or_else(|| format!("{url}: no host"))?.trim_start_matches('[').trim_end_matches(']').to_string();
        let tls = uri.scheme_str() == Some("wss");
        let port = uri.port_u16().unwrap_or(if tls { 443 } else { 80 });
        let addrs = (host.as_str(), port).to_socket_addrs().map_err(|e| format!("{host}: {e}"))?;
        let mut last = format!("{host}: no address");
        let mut stream = None;
        for a in addrs {
            match TcpStream::connect_timeout(&a, CONNECT_TIMEOUT) {
                Ok(s) => {
                    stream = Some(s);
                    break;
                }
                Err(e) => last = format!("{host}:{port}: {e}"),
            }
        }
        let stream = stream.ok_or(last)?;
        let _ = stream.set_nodelay(true);
        stream.set_read_timeout(Some(CONNECT_TIMEOUT)).map_err(|e| e.to_string())?;
        stream.set_write_timeout(Some(CONNECT_TIMEOUT)).map_err(|e| e.to_string())?;
        if tls {
            // Several crypto providers can be compiled in: pick ring unless one is already chosen
            // (installing twice is an error to ignore, not a failure).
            let _ = rustls::crypto::ring::default_provider().install_default();
        }
        let (ws, _) = tungstenite::client_tls_with_config(request, stream, None, None).map_err(|e| e.to_string())?;
        // From here on reads time out so the thread can send between them.
        let tcp = match ws.get_ref() {
            MaybeTlsStream::Plain(s) => Some(s),
            MaybeTlsStream::Rustls(s) => Some(s.get_ref()),
            _ => None,
        };
        if let Some(s) = tcp {
            s.set_read_timeout(Some(POLL)).map_err(|e| e.to_string())?;
        }
        Ok(ws)
    }
}

#[cfg(target_arch = "wasm32")]
mod web {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    use wasm_bindgen::JsCast as _;
    use wasm_bindgen::closure::Closure;

    use super::{Event, Socket, backoff_ms};

    type Queue = Rc<RefCell<VecDeque<Event>>>;

    /// The browser's WebSocket, reopened after it closes.
    pub struct WebSocket {
        url: String,
        repaint: Option<egui::Context>,
        queue: Queue,
        ws: Option<web_sys::WebSocket>,
        /// The handlers live as long as the socket they're installed on.
        handlers: Vec<Box<dyn std::any::Any>>,
        attempt: u32,
        /// When to reconnect (ms, `Date.now()`), while closed.
        retry_at: Option<f64>,
    }

    fn push(queue: &Queue, repaint: &Option<egui::Context>, e: Event) {
        // The handlers run from the browser's event loop, never inside `poll`: the queue is free.
        if let Ok(mut q) = queue.try_borrow_mut() {
            q.push_back(e);
        }
        if let Some(ctx) = repaint {
            ctx.request_repaint();
        }
    }

    impl WebSocket {
        pub fn open(url: &str, repaint: Option<egui::Context>) -> Self {
            let mut s = Self { url: url.to_string(), repaint, queue: Queue::default(), ws: None, handlers: vec![], attempt: 0, retry_at: None };
            s.reopen();
            s
        }

        fn reopen(&mut self) {
            self.detach();
            self.retry_at = None;
            let ws = match web_sys::WebSocket::new(&self.url) {
                Ok(ws) => ws,
                Err(e) => {
                    let msg = format!("{}: {}", self.url, js_text(&e));
                    push(&self.queue, &self.repaint, Event::Error(msg.clone()));
                    push(&self.queue, &self.repaint, Event::Closed(msg));
                    return;
                }
            };
            ws.set_binary_type(web_sys::BinaryType::Arraybuffer);
            let (q, r) = (self.queue.clone(), self.repaint.clone());
            let on_open = Closure::<dyn FnMut(web_sys::Event)>::new(move |_: web_sys::Event| push(&q, &r, Event::Opened));
            let (q, r) = (self.queue.clone(), self.repaint.clone());
            let on_message = Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |e: web_sys::MessageEvent| {
                if let Ok(buf) = e.data().dyn_into::<js_sys::ArrayBuffer>() {
                    push(&q, &r, Event::Message(js_sys::Uint8Array::new(&buf).to_vec()));
                }
            });
            let (q, r) = (self.queue.clone(), self.repaint.clone());
            let on_close = Closure::<dyn FnMut(web_sys::CloseEvent)>::new(move |e: web_sys::CloseEvent| {
                let reason = e.reason();
                let why = if reason.is_empty() { format!("connection closed ({})", e.code()) } else { format!("closed: {} {reason}", e.code()) };
                push(&q, &r, Event::Closed(why));
            });
            let (q, r) = (self.queue.clone(), self.repaint.clone());
            let on_error = Closure::<dyn FnMut(web_sys::Event)>::new(move |_: web_sys::Event| {
                push(&q, &r, Event::Error("couldn't reach the collaboration server".into()));
            });
            ws.set_onopen(Some(on_open.as_ref().unchecked_ref()));
            ws.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
            ws.set_onclose(Some(on_close.as_ref().unchecked_ref()));
            ws.set_onerror(Some(on_error.as_ref().unchecked_ref()));
            self.handlers = vec![Box::new(on_open), Box::new(on_message), Box::new(on_close), Box::new(on_error)];
            self.ws = Some(ws);
        }

        /// Unhook and close the current socket.
        fn detach(&mut self) {
            if let Some(ws) = self.ws.take() {
                ws.set_onopen(None);
                ws.set_onmessage(None);
                ws.set_onclose(None);
                ws.set_onerror(None);
                // Closing an already closed socket is fine to ignore.
                let _ = ws.close();
            }
            self.handlers.clear();
        }
    }

    impl Socket for WebSocket {
        fn send(&mut self, msg: Vec<u8>) {
            if let Some(ws) = &self.ws
                && ws.ready_state() == web_sys::WebSocket::OPEN
            {
                // A failed send shows up as a close event.
                let _ = ws.send_with_u8_array(&msg);
            }
        }

        fn poll(&mut self) -> Vec<Event> {
            let events: Vec<Event> = self.queue.try_borrow_mut().map(|mut q| q.drain(..).collect()).unwrap_or_default();
            let now = js_sys::Date::now();
            for e in &events {
                match e {
                    Event::Opened => self.attempt = 0,
                    Event::Closed(_) if self.retry_at.is_none() => {
                        self.retry_at = Some(now + backoff_ms(self.attempt) as f64);
                        self.attempt = self.attempt.saturating_add(1);
                    }
                    _ => {}
                }
            }
            if self.retry_at.is_some_and(|t| now >= t) {
                self.reopen();
            }
            events
        }
    }

    impl Drop for WebSocket {
        fn drop(&mut self) {
            self.detach();
        }
    }

    fn js_text(v: &wasm_bindgen::JsValue) -> String {
        v.as_string().unwrap_or_else(|| format!("{v:?}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_and_caps() {
        assert_eq!(backoff_ms(0), BACKOFF_MIN_MS);
        assert_eq!(backoff_ms(1), 2 * BACKOFF_MIN_MS);
        assert_eq!(backoff_ms(40), BACKOFF_MAX_MS);
        assert_eq!(backoff_ms(u32::MAX), BACKOFF_MAX_MS);
    }

    #[test]
    fn native_socket_reports_an_unreachable_server_and_retries() {
        // Port 1 on localhost refuses connections: the socket reports the failure without
        // blocking the caller.
        let mut s = connect("ws://127.0.0.1:1/collab/test", None);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut seen = vec![];
        while std::time::Instant::now() < deadline && !seen.iter().any(|e| matches!(e, Event::Closed(_))) {
            seen.extend(s.poll());
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(seen.iter().any(|e| matches!(e, Event::Closed(_))), "{seen:?}");
        // Sending while down is dropped quietly.
        s.send(vec![1, 2, 3]);
    }

    #[test]
    fn native_socket_reports_a_failed_tls_handshake() {
        // A server that hangs up on the TLS hello: the wss path fails cleanly.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for c in listener.incoming().take(2) {
                drop(c);
            }
        });
        let mut s = connect(&format!("wss://127.0.0.1:{port}/collab/test"), None);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut seen = vec![];
        while std::time::Instant::now() < deadline && !seen.iter().any(|e| matches!(e, Event::Closed(_))) {
            seen.extend(s.poll());
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(seen.iter().any(|e| matches!(e, Event::Error(_))), "{seen:?}");
        assert!(!seen.contains(&Event::Opened), "{seen:?}");
    }
}
