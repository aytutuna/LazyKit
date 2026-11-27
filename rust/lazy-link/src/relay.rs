//! The client of the Lazy relay: paired devices that are not on the same network meet in a room
//! there and the phone link's connections run through it as streams.
//! Required Notice: Copyright (c) 2026 Alazy Inc. (https://alazy.app)

use futures_util::{SinkExt, StreamExt};
use rustls_platform_verifier::BuilderVerifierExt;
use sha2::{Digest, Sha256};
use std::fmt;
use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream, ReadBuf};
use tokio::net::TcpStream;
use tokio::sync::{Notify, mpsc};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::{self, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

/// The largest frame the relay forwards; a larger one closes the stream.
pub const MAX_FRAME: usize = 64 * 1024;
/// Streams open at once in one room.
pub const MAX_STREAMS: usize = 16;
/// What the relay may hold for a receiver before it closes the stream with [`CLOSE_TOO_SLOW`].
pub const MAX_BUFFERED: usize = 8 * 1024 * 1024;
pub const CLOSE_TOO_SLOW: u16 = 1013;
/// The weekly cap closes streams with this application code; 1008 is a protocol fault.
pub const CLOSE_CAPPED: u16 = 4402;

const KEEPALIVE: Duration = Duration::from_secs(30);
/// The wait between tries while a woken host starts; it grows from the first to the second.
const WAKE_POLL: Duration = Duration::from_secs(1);
const WAKE_POLL_SLOW: Duration = Duration::from_secs(4);
/// A larger Retry-After from the relay or anything in front of it is cut to this many seconds.
const MAX_RETRY_AFTER: u64 = 3600;
/// A connection that has said nothing for this long, not even the relay's answer to a ping, is dead.
const DEAD: Duration = Duration::from_secs(60);
/// How long a connect or a plain request may take before it counts as failed.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// How much a stream holds before a write waits.
const WINDOW: usize = 256 * 1024;
/// Frames a stream may have sent that the other end has not yet handed to its reader. The other
/// end answers each frame it has delivered with an `ack`; without this the relay would take
/// frames from a fast writer until a slow reader's backlog passed [`MAX_BUFFERED`] and cut the
/// stream. The window times [`MAX_FRAME`] stays under that limit.
pub const WINDOW_FRAMES: usize = 64;
const _: () = assert!(WINDOW_FRAMES * MAX_FRAME < MAX_BUFFERED);

fn keepalive() -> tokio::time::Interval {
    tokio::time::interval_at(tokio::time::Instant::now() + KEEPALIVE, KEEPALIVE)
}

// What the reading half of a stream tells the writing half.
enum Ctl {
    /// The other end delivered a frame, so one more may be sent.
    Credit,
    /// A frame arrived and went to the reader; the other end is told so.
    Ack,
}

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

#[derive(Debug)]
pub enum RelayError {
    /// The host of the room is not connected; `woken` says the relay sent its device a push.
    NoHost {
        woken: bool,
    },
    /// The group has used its week's allowance and has not paid.
    Capped,
    TooManyStreams,
    /// This address has made too many requests; try again after `retry_after`.
    RateLimited {
        retry_after: Duration,
    },
    /// The room belongs to another group.
    WrongGroup,
    /// The purchase already pays for another group.
    PurchaseUsed,
    /// The relay cannot check this store's receipts yet.
    NotConfigured,
    ReceiptRejected,
    Refused(u16),
    /// The host's control connection ended with this close code (1006 when it just dropped).
    Closed(u16),
    Transport(String),
}

impl fmt::Display for RelayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoHost { woken } => write!(f, "nobody is waiting in the room (woken: {woken})"),
            Self::Capped => f.write_str("the group has used its weekly allowance"),
            Self::TooManyStreams => f.write_str("too many streams in the room"),
            Self::RateLimited { retry_after } => {
                write!(f, "too many requests, retry in {} s", retry_after.as_secs())
            }
            Self::WrongGroup => f.write_str("the room belongs to another group"),
            Self::PurchaseUsed => f.write_str("the purchase pays for another group"),
            Self::NotConfigured => f.write_str("the relay cannot check receipts of this store yet"),
            Self::ReceiptRejected => f.write_str("the store rejected the receipt"),
            Self::Refused(status) => write!(f, "the relay answered {status}"),
            Self::Closed(code) => write!(f, "the relay closed the connection ({code})"),
            Self::Transport(e) => f.write_str(e),
        }
    }
}

impl std::error::Error for RelayError {}

impl From<tungstenite::Error> for RelayError {
    fn from(e: tungstenite::Error) -> Self {
        let tungstenite::Error::Http(res) = &e else {
            return Self::Transport(e.to_string());
        };
        match res.status().as_u16() {
            404 => Self::NoHost {
                woken: res.headers().contains_key("x-relay-woken"),
            },
            402 => Self::Capped,
            403 => Self::WrongGroup,
            429 => Self::TooManyStreams,
            503 => match retry_after(
                res.headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok()),
            ) {
                Some(retry_after) => Self::RateLimited { retry_after },
                None => Self::Refused(503),
            },
            status => Self::Refused(status),
        }
    }
}

// The relay answers its per-address limit with 503 and a Retry-After in seconds.
fn retry_after(value: Option<&str>) -> Option<Duration> {
    let secs: u64 = value?.trim().parse().ok()?;
    Some(Duration::from_secs(secs.min(MAX_RETRY_AFTER)))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Apns,
    Fcm,
}

impl Platform {
    fn as_str(self) -> &'static str {
        match self {
            Self::Apns => "apns",
            Self::Fcm => "fcm",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Store {
    Apple,
    Google,
    Microsoft,
}

impl Store {
    fn as_str(self) -> &'static str {
        match self {
            Self::Apple => "apple",
            Self::Google => "google",
            Self::Microsoft => "microsoft",
        }
    }
}

/// What the relay has counted for a group this week.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupStatus {
    pub used: u64,
    pub cap: u64,
    /// When the paid time ends, in milliseconds since the epoch; 0 for a group that never paid.
    pub paid_until: u64,
    pub paid: bool,
    pub capped: bool,
}

/// A random 32-digit hex id, the form of both group and device ids.
pub fn random_id() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("the system has no random source");
    hex(&bytes)
}

/// The room two devices meet in, named from their pairing secret so the relay learns nothing else.
pub fn room_id(secret: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(b"lazy relay room v1");
    h.update(secret);
    hex(&h.finalize())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// One device's view of the relay: its group and its own id.
#[derive(Debug, Clone)]
pub struct Relay {
    base: String,
    group: String,
    device: String,
}

impl Relay {
    /// `base` is the relay's address up to the version, for example `https://alazy.app/relay/v1`.
    pub fn new(base: &str, group: &str, device: &str) -> Self {
        Self {
            base: base.trim_end_matches('/').to_string(),
            group: group.into(),
            device: device.into(),
        }
    }

    pub async fn status(&self) -> Result<GroupStatus, RelayError> {
        let (status, body) = self.api("GET", "", None).await?;
        status_of(status, &body)
    }

    pub async fn register_push(&self, platform: Platform, token: &str) -> Result<(), RelayError> {
        let body = serde_json::json!({ "device": self.device, "platform": platform.as_str(), "token": token });
        match self.api("POST", "/push", Some(body.to_string())).await? {
            (204, _) => Ok(()),
            (status, _) => Err(RelayError::Refused(status)),
        }
    }

    pub async fn drop_push(&self) -> Result<(), RelayError> {
        match self
            .api("DELETE", &format!("/push/{}", self.device), None)
            .await?
        {
            (204, _) => Ok(()),
            (status, _) => Err(RelayError::Refused(status)),
        }
    }

    /// Hands the relay a store's receipt; once it checks out the whole group is past the cap.
    pub async fn pay(&self, store: Store, receipt: &str) -> Result<GroupStatus, RelayError> {
        let body = serde_json::json!({ "store": store.as_str(), "receipt": receipt });
        let (status, body) = self.api("POST", "/paid", Some(body.to_string())).await?;
        match status {
            501 => Err(RelayError::NotConfigured),
            422 => Err(RelayError::ReceiptRejected),
            409 => Err(RelayError::PurchaseUsed),
            _ => status_of(status, &body),
        }
    }

    /// Waits in the room for guests; every one that connects comes out of [`RelayHost::accept`].
    pub async fn host(&self, room: &str) -> Result<RelayHost, RelayError> {
        let ws = self
            .socket(
                room,
                &format!("role=host&device={}", self.device),
                REQUEST_TIMEOUT,
            )
            .await?;
        let (tx, opens) = mpsc::channel(MAX_STREAMS);
        let task = tokio::spawn(control(ws, tx, self.clone(), room.into()));
        Ok(RelayHost { opens, task })
    }

    /// Opens one stream to the host of the room.
    pub async fn connect(&self, room: &str) -> Result<RelayStream, RelayError> {
        self.connect_within(room, REQUEST_TIMEOUT).await
    }

    async fn connect_within(
        &self,
        room: &str,
        within: Duration,
    ) -> Result<RelayStream, RelayError> {
        let ws = self
            .socket(room, &format!("role=guest&device={}", self.device), within)
            .await?;
        Ok(RelayStream::new(ws))
    }

    /// Like [`connect`](Self::connect), but when the relay has woken the host's closed app it
    /// tries again, a little slower each time, until `timeout` has passed.
    pub async fn connect_waiting(
        &self,
        room: &str,
        timeout: Duration,
    ) -> Result<RelayStream, RelayError> {
        let deadline = tokio::time::Instant::now() + timeout;
        let mut poll = WAKE_POLL;
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            match self.connect_within(room, left.min(REQUEST_TIMEOUT)).await {
                Err(RelayError::NoHost { woken: true })
                    if tokio::time::Instant::now() + poll < deadline =>
                {
                    tokio::time::sleep(poll).await;
                    poll = (poll * 2).min(WAKE_POLL_SLOW);
                }
                Err(RelayError::RateLimited { retry_after })
                    if tokio::time::Instant::now() + retry_after < deadline =>
                {
                    tokio::time::sleep(retry_after.max(WAKE_POLL)).await;
                }
                other => return other,
            }
        }
    }

    async fn socket(
        &self,
        room: &str,
        query: &str,
        within: Duration,
    ) -> Result<Socket, RelayError> {
        let base = self.base.replacen("http", "ws", 1);
        let url = if query.starts_with("role=stream") {
            format!("{base}/{room}?{query}")
        } else {
            format!("{base}/{room}?{query}&group={}", self.group)
        };
        let connector = tokio_tungstenite::Connector::Rustls(Arc::new(tls_config()?));
        let connecting =
            tokio_tungstenite::connect_async_tls_with_config(url, None, false, Some(connector));
        match tokio::time::timeout(within, connecting).await {
            Ok(res) => Ok(res?.0),
            Err(_) => Err(RelayError::Transport(
                "the relay did not answer in time".into(),
            )),
        }
    }

    async fn api(
        &self,
        method: &'static str,
        path: &str,
        body: Option<String>,
    ) -> Result<(u16, String), RelayError> {
        let url = format!("{}/group/{}{path}", self.base, self.group);
        tokio::task::spawn_blocking(move || {
            let agent: ureq::Agent = ureq::Agent::config_builder()
                .http_status_as_error(false)
                .timeout_global(Some(REQUEST_TIMEOUT))
                .tls_config(
                    ureq::tls::TlsConfig::builder()
                        .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                        .build(),
                )
                .build()
                .into();
            let res = match (method, body) {
                ("GET", _) => agent.get(&url).call(),
                ("DELETE", _) => agent.delete(&url).call(),
                (_, body) => agent
                    .post(&url)
                    .content_type("application/json")
                    .send(body.unwrap_or_default()),
            };
            let mut res = res.map_err(|e| RelayError::Transport(e.to_string()))?;
            let status = res.status().as_u16();
            if status == 503
                && let Some(retry_after) = retry_after(
                    res.headers()
                        .get("retry-after")
                        .and_then(|v| v.to_str().ok()),
                )
            {
                return Err(RelayError::RateLimited { retry_after });
            }
            let body = res.body_mut().read_to_string().unwrap_or_default();
            Ok((status, body))
        })
        .await
        .map_err(|e| RelayError::Transport(e.to_string()))?
    }
}

// The same system trust store the HTTP calls use. rustls-native-certs finds none on iPhone or
// Android, so the socket cannot rely on tokio-tungstenite's own roots.
fn tls_config() -> Result<rustls::ClientConfig, RelayError> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .and_then(|b| b.with_platform_verifier())
        .map(|b| b.with_no_client_auth())
        .map_err(|e| RelayError::Transport(e.to_string()))
}

fn status_of(status: u16, body: &str) -> Result<GroupStatus, RelayError> {
    if status != 200 {
        return Err(RelayError::Refused(status));
    }
    let v: serde_json::Value =
        serde_json::from_str(body).map_err(|e| RelayError::Transport(e.to_string()))?;
    let num = |k: &str| v[k].as_u64().unwrap_or(0);
    let flag = |k: &str| v[k].as_bool().unwrap_or(false);
    Ok(GroupStatus {
        used: num("used"),
        cap: num("cap"),
        paid_until: num("paidUntil"),
        paid: flag("paid"),
        capped: flag("capped"),
    })
}

/// The host's end of a room. Dropping it leaves the room.
pub struct RelayHost {
    opens: mpsc::Receiver<Result<RelayStream, RelayError>>,
    task: JoinHandle<()>,
}

impl RelayHost {
    /// Waits for the next guest and returns the host's end of its stream. An error means the
    /// control connection is gone; a guest whose stream could not be joined is skipped.
    pub async fn accept(&mut self) -> Result<RelayStream, RelayError> {
        self.opens
            .recv()
            .await
            .unwrap_or(Err(RelayError::Closed(1006)))
    }
}

impl Drop for RelayHost {
    fn drop(&mut self) {
        self.task.abort();
    }
}

// Reads the control socket for guests that arrived and keeps it alive. The relay answers every
// ping, so a socket that has gone quiet for too long is dead even if no error has come yet.
async fn control(
    mut ws: Socket,
    tx: mpsc::Sender<Result<RelayStream, RelayError>>,
    relay: Relay,
    room: String,
) {
    let mut ping = keepalive();
    let mut heard = tokio::time::Instant::now();
    loop {
        tokio::select! {
            msg = ws.next() => {
                heard = tokio::time::Instant::now();
                match msg {
                    Some(Ok(Message::Text(t))) => {
                        if let Some(id) = t.strip_prefix("open ") {
                            // Each guest gets its own task, so a slow handshake does not hold up the next.
                            tokio::spawn(join(relay.clone(), room.clone(), id.to_string(), tx.clone()));
                        }
                    }
                    Some(Ok(Message::Close(frame))) => {
                        let code = frame.map_or(1005, |f| u16::from(f.code));
                        let _ = tx.send(Err(RelayError::Closed(code))).await;
                        return;
                    }
                    Some(Ok(_)) => {}
                    Some(Err(_)) | None => {
                        let _ = tx.send(Err(RelayError::Closed(1006))).await;
                        return;
                    }
                }
            },
            _ = ping.tick() => {
                if heard.elapsed() > DEAD || ws.send(Message::text("ping")).await.is_err() {
                    let _ = tx.send(Err(RelayError::Closed(1006))).await;
                    return;
                }
            }
        }
    }
}

async fn join(
    relay: Relay,
    room: String,
    id: String,
    tx: mpsc::Sender<Result<RelayStream, RelayError>>,
) {
    let query = format!("role=stream&id={id}");
    // A refused join (the relay forgets one after ten seconds, or limits the address) costs
    // only that guest, who gives up by itself; the control connection is still good.
    if let Ok(ws) = relay.socket(&room, &query, REQUEST_TIMEOUT).await {
        let _ = tx.send(Ok(RelayStream::new(ws))).await;
    }
}

/// One connection of the phone link through the relay, as a byte stream. A clean close from
/// the other end reads as end of input; any other close is available from
/// [`close_code`](Self::close_code), and reads then end and writes fail.
///
/// `shutdown` is a half-close, as on TCP: the other end reads end of input, and this end can
/// still read the reply. Dropping the stream closes both directions, but what was written
/// before is still sent first, as TCP would, for as long as the other end keeps taking it.
pub struct RelayStream {
    io: DuplexStream,
    close: Arc<OnceLock<u16>>,
    dropped: Arc<(AtomicBool, Notify)>,
}

// What the socket's reading task hands to the task that delivers to the reader.
enum Inbound {
    Data(Vec<u8>),
    Eof,
}

impl RelayStream {
    fn new(ws: Socket) -> Self {
        let (local, remote) = tokio::io::duplex(WINDOW);
        let (mut rd, mut wr) = tokio::io::split(remote);
        let (mut sink, mut source) = ws.split();
        let (ctl, mut ctl_rx) = mpsc::unbounded_channel();
        let close = Arc::new(OnceLock::new());
        let dropped = Arc::new((AtomicBool::new(false), Notify::new()));

        let gone = dropped.clone();
        let outbound = tokio::spawn(async move {
            let mut ping = keepalive();
            let mut buf = vec![0u8; MAX_FRAME];
            let mut writing = true;
            let mut credit = WINDOW_FRAMES;
            let mut acks = true;
            let mut moved = tokio::time::Instant::now();
            loop {
                // Nothing more can be sent once the credit is spent and no answer can come.
                if credit == 0 && !acks {
                    let _ = sink.send(Message::Close(None)).await;
                    return;
                }
                tokio::select! {
                    n = rd.read(&mut buf), if writing && credit > 0 => match n {
                        Ok(0) | Err(_) if gone.0.load(Ordering::Acquire) => {
                            let _ = sink.send(Message::Close(None)).await;
                            return;
                        }
                        Ok(0) | Err(_) => {
                            // The writer is done; the reader may still be waiting for a reply.
                            writing = false;
                            if sink.send(Message::text("eof")).await.is_err() {
                                return;
                            }
                        }
                        Ok(n) => {
                            credit -= 1;
                            moved = tokio::time::Instant::now();
                            if sink.send(Message::binary(buf[..n].to_vec())).await.is_err() {
                                return;
                            }
                        }
                    },
                    c = ctl_rx.recv(), if acks => match c {
                        Some(Ctl::Credit) => {
                            credit += 1;
                            moved = tokio::time::Instant::now();
                        }
                        Some(Ctl::Ack) => {
                            if sink.send(Message::text("ack")).await.is_err() {
                                return;
                            }
                        }
                        None => acks = false,
                    },
                    // Dropped after a shutdown: nothing is left to read, so say goodbye.
                    _ = gone.1.notified(), if !writing => {
                        let _ = sink.send(Message::Close(None)).await;
                        return;
                    }
                    _ = ping.tick() => {
                        // A dropped stream whose data the other end has stopped taking is given up on.
                        if gone.0.load(Ordering::Acquire) && moved.elapsed() > DEAD {
                            let _ = sink.send(Message::Close(None)).await;
                            return;
                        }
                        if sink.send(Message::text("ping")).await.is_err() {
                            return;
                        }
                    }
                }
            }
        })
        .abort_handle();

        // Frames wait here for the reader, so an ack from the other end is never stuck behind
        // data nobody has read yet. The other end sends at most WINDOW_FRAMES before one is
        // acknowledged, and a frame is acknowledged once it is delivered.
        let (queue, mut queued) = mpsc::channel(2 * WINDOW_FRAMES);
        let credit_ctl = ctl.clone();
        let gone = dropped.clone();
        tokio::spawn(async move {
            while let Some(item) = queued.recv().await {
                match item {
                    Inbound::Data(b) => {
                        // After a drop nobody reads, but the other end still gets its acks.
                        if !gone.0.load(Ordering::Acquire) {
                            let _ = wr.write_all(&b).await;
                        }
                        let _ = ctl.send(Ctl::Ack);
                    }
                    Inbound::Eof => {
                        let _ = wr.shutdown().await;
                    }
                }
            }
        });

        let stop = outbound.clone();
        let code = close.clone();
        tokio::spawn(async move {
            loop {
                // Pings are answered, so any silence this long means the path is dead.
                let Ok(msg) = tokio::time::timeout(DEAD, source.next()).await else {
                    let _ = code.set(1006);
                    break;
                };
                match msg {
                    Some(Ok(Message::Binary(b))) => {
                        if queue.try_send(Inbound::Data(b.to_vec())).is_err() {
                            let _ = code.set(1008);
                            break;
                        }
                    }
                    Some(Ok(Message::Text(t))) if t.as_str() == "ack" => {
                        let _ = credit_ctl.send(Ctl::Credit);
                    }
                    Some(Ok(Message::Text(t))) if t.as_str() == "eof" => {
                        let _ = queue.try_send(Inbound::Eof);
                    }
                    Some(Ok(Message::Close(frame))) => {
                        let _ = code.set(frame.map_or(1005, |f| u16::from(f.code)));
                        break;
                    }
                    Some(Ok(_)) => {}
                    Some(Err(_)) => {
                        let _ = code.set(1006);
                        break;
                    }
                    None => break,
                }
            }
            let _ = code.set(1006);
            stop.abort();
        });

        Self {
            io: local,
            close,
            dropped,
        }
    }

    /// The code the stream ended with, once it has ended: 1000 for a normal close, 1013 when
    /// the relay cut it off because a reader fell behind, 4402 at the weekly cap, 1006 when the
    /// connection simply dropped.
    pub fn close_code(&self) -> Option<u16> {
        self.close.get().copied()
    }
}

impl Drop for RelayStream {
    fn drop(&mut self) {
        // The writer task sends what is still in the pipe and then says goodbye to the relay;
        // the reading tasks go on until the relay answers, so credit keeps coming back.
        self.dropped.0.store(true, Ordering::Release);
        self.dropped.1.notify_one();
    }
}

impl AsyncRead for RelayStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_read(cx, buf)
    }
}

impl AsyncWrite for RelayStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.io).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.io).poll_shutdown(cx)
    }
}
