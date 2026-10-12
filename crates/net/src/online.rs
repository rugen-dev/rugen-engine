//! Client for `rugen-server`'s JSON API. Blocking calls over std TCP with a
//! short timeout, plus [`Pending`] to run any call on a worker thread so the
//! game loop never stalls.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::Duration;

pub use rugen_proto::{
    Leaderboard, MatchInfo, ProfileInfo, QueueStatus, Registered, ResultAck, RoomInfo, Season,
    ServerInfo, rank_name,
};
use serde_json::{Value, json};

#[derive(Debug, thiserror::Error)]
pub enum OnlineError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("server: {0}")]
    Server(String),
    #[error("bad reply: {0}")]
    Decode(String),
    #[error("no account key")]
    NoKey,
}

pub type OnlineResult<T> = std::result::Result<T, OnlineError>;

#[derive(Clone, Debug)]
pub struct OnlineClient {
    /// `host:port` of the TCP API.
    pub server: String,
    pub key: Option<String>,
    pub timeout: Duration,
}

impl OnlineClient {
    pub fn new(server: &str, key: Option<String>) -> Self {
        Self {
            server: server.to_string(),
            key: key.filter(|k| !k.is_empty()),
            timeout: Duration::from_secs(5),
        }
    }

    fn addr(&self) -> OnlineResult<SocketAddr> {
        self.server
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| OnlineError::Server(format!("cannot resolve {}", self.server)))
    }

    /// UDP rendezvous endpoint: same host as the API, the port the server reports.
    pub fn udp_addr(&self, udp_port: u16) -> OnlineResult<SocketAddr> {
        let mut a = self.addr()?;
        a.set_port(udp_port);
        Ok(a)
    }

    fn call(&self, method: &str, path: &str, body: Option<Value>) -> OnlineResult<Value> {
        let addr = self.addr()?;
        let mut stream = TcpStream::connect_timeout(&addr, self.timeout)?;
        stream.set_read_timeout(Some(self.timeout))?;
        stream.set_write_timeout(Some(self.timeout))?;
        let host = self.server.split(':').next().unwrap_or("rugen");
        let payload = body.map(|b| serde_json::to_vec(&b).unwrap_or_default());
        let mut head = format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n");
        if let Some(p) = &payload {
            head.push_str(&format!(
                "Content-Type: application/json\r\nContent-Length: {}\r\n",
                p.len()
            ));
        }
        head.push_str("\r\n");
        stream.write_all(head.as_bytes())?;
        if let Some(p) = &payload {
            stream.write_all(p)?;
        }
        let mut raw = Vec::new();
        stream.take(1 << 20).read_to_end(&mut raw)?;
        let text = String::from_utf8_lossy(&raw);
        let (status_line, rest) = text
            .split_once("\r\n")
            .ok_or_else(|| OnlineError::Decode("empty reply".into()))?;
        let status: u16 = status_line
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let body = rest.split_once("\r\n\r\n").map_or("", |(_, b)| b);
        let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
        if status != 200 {
            let msg = v
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("http {status}"));
            return Err(OnlineError::Server(msg));
        }
        Ok(v)
    }

    fn decode<T: serde::de::DeserializeOwned>(v: Value) -> OnlineResult<T> {
        serde_json::from_value(v).map_err(|e| OnlineError::Decode(e.to_string()))
    }

    fn key(&self) -> OnlineResult<&str> {
        self.key.as_deref().ok_or(OnlineError::NoKey)
    }

    fn with_key(&self, mut body: Value) -> OnlineResult<Value> {
        body["key"] = Value::String(self.key()?.to_string());
        Ok(body)
    }

    pub fn info(&self) -> OnlineResult<ServerInfo> {
        Self::decode(self.call("GET", "/", None)?)
    }

    /// Create an anonymous account; the key is stored in the returned client.
    pub fn register(&mut self) -> OnlineResult<Registered> {
        let r: Registered = Self::decode(self.call("POST", "/register", Some(json!({})))?)?;
        self.key = Some(r.key.clone());
        Ok(r)
    }

    pub fn set_profile(&self, name: &str, avatar: u32) -> OnlineResult<ProfileInfo> {
        let b = self.with_key(json!({ "name": name, "avatar": avatar }))?;
        Self::decode(self.call("POST", "/profile", Some(b))?)
    }

    pub fn profile(&self, id: u64) -> OnlineResult<ProfileInfo> {
        Self::decode(self.call("GET", &format!("/profile/{id}"), None)?)
    }

    pub fn create_room(&self, char_id: &str, stage: &str) -> OnlineResult<RoomInfo> {
        let b = self.with_key(json!({ "char": char_id, "stage": stage }))?;
        Self::decode(self.call("POST", "/room", Some(b))?)
    }

    pub fn join_room(&self, code: &str, char_id: &str) -> OnlineResult<RoomInfo> {
        let b = self.with_key(json!({ "char": char_id }))?;
        Self::decode(self.call(
            "POST",
            &format!("/room/{}/join", code.trim().to_ascii_uppercase()),
            Some(b),
        )?)
    }

    pub fn poll_room(&self, code: &str) -> OnlineResult<RoomInfo> {
        let k = self.key()?;
        Self::decode(self.call(
            "GET",
            &format!("/room/{}?key={k}", code.trim().to_ascii_uppercase()),
            None,
        )?)
    }

    pub fn enter_queue(&self, char_id: &str) -> OnlineResult<QueueStatus> {
        let b = self.with_key(json!({ "char": char_id }))?;
        Self::decode(self.call("POST", "/queue", Some(b))?)
    }

    pub fn poll_queue(&self) -> OnlineResult<QueueStatus> {
        let k = self.key()?;
        Self::decode(self.call("GET", &format!("/queue?key={k}"), None)?)
    }

    pub fn leave_queue(&self) -> OnlineResult<()> {
        let b = self.with_key(json!({}))?;
        self.call("POST", "/queue/leave", Some(b))?;
        Ok(())
    }

    pub fn report(&self, match_id: u64, winner: u64) -> OnlineResult<ResultAck> {
        let b = self.with_key(json!({ "match_id": match_id, "winner": winner }))?;
        Self::decode(self.call("POST", "/result", Some(b))?)
    }

    pub fn leaderboard(&self) -> OnlineResult<Leaderboard> {
        let path = match &self.key {
            Some(k) => format!("/leaderboard?key={k}"),
            None => "/leaderboard".into(),
        };
        Self::decode(self.call("GET", &path, None)?)
    }

    pub fn season(&self) -> OnlineResult<Season> {
        Self::decode(self.call("GET", "/seasons/current", None)?)
    }
}

/// A call running on a worker thread; poll with [`Pending::take`].
pub struct Pending<T> {
    rx: Receiver<OnlineResult<T>>,
}

impl<T: Send + 'static> Pending<T> {
    pub fn spawn(f: impl FnOnce() -> OnlineResult<T> + Send + 'static) -> Self {
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
        Self { rx }
    }

    /// The result once it arrived; `None` while still running (or if the worker vanished).
    pub fn take(&mut self) -> Option<OnlineResult<T>> {
        match self.rx.try_recv() {
            Ok(r) => Some(r),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                Some(Err(OnlineError::Server("request thread died".into())))
            }
        }
    }
}
