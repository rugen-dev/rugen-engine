//! Client for `rugen-server`'s JSON API over HTTP(S), plus [`Pending`] to run
//! any call on a worker thread so the game loop never stalls.
//!
//! `server` is a base URL (`https://rugen.example.com`, typically behind a
//! Cloudflare Tunnel) or, for older setups, a bare `host:port` (plain HTTP).

use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::{Duration, Instant};

pub use rugen_proto::{
    AccountInfo, CharRating, FairnessProfile, FighterCard, FighterDecl, FighterEntry, Leaderboard,
    MatchInfo, MatchupView, PackageStatus, ProfileInfo, QueueStatus, ReadyReport, Registered,
    Replay, ResultAck, RoomInfo, RoomMember, RoomSummary, Rules, Season, SeasonBadge, ServerInfo,
    ServerMsg, Session, SessionTicket, SignedBlob, TruthFeed, Verdict, VerdictKind, b64_decode,
    ping_bars, rank_name,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Upload / download chunk size (the server accepts up to 4 MB).
pub const CHUNK: usize = 2 << 20;

/// Progress of a package transfer, shared with the UI thread.
#[derive(Debug, Default)]
pub struct Transfer {
    pub done: AtomicU64,
    pub total: AtomicU64,
    pub cancel: AtomicBool,
}

impl Transfer {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// 0.0 ..= 1.0
    pub fn fraction(&self) -> f32 {
        let t = self.total.load(Ordering::Relaxed);
        if t == 0 {
            0.0
        } else {
            (self.done.load(Ordering::Relaxed) as f32 / t as f32).min(1.0)
        }
    }
}
use rugen_proto::API_HEADER;
use serde_json::{Value, json};

/// Every request says which lobby API it speaks ([`API_HEADER`]): servers
/// serving several APIs keep players of different versions apart.
const API: u32 = rugen_proto::API_VERSION;

#[derive(Debug, thiserror::Error)]
pub enum OnlineError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Http(String),
    #[error("server: {0}")]
    Server(String),
    #[error("bad reply: {0}")]
    Decode(String),
    #[error("no account key")]
    NoKey,
    #[error("no server set ([net] server in rugen.toml)")]
    NoServer,
}

pub type OnlineResult<T> = std::result::Result<T, OnlineError>;

/// `https://host/` → `https://host`; `host:port` → `http://host:port`.
pub fn base_url(server: &str) -> String {
    let s = server.trim().trim_end_matches('/');
    if s.contains("://") {
        s.to_string()
    } else {
        format!("http://{s}")
    }
}

/// Host part of a base URL (no scheme, port or path).
pub fn url_host(base: &str) -> &str {
    let rest = base.split("://").nth(1).unwrap_or(base);
    let hostport = rest.split('/').next().unwrap_or(rest);
    if let Some(v6) = hostport.strip_prefix('[') {
        return v6.split(']').next().unwrap_or(v6);
    }
    hostport.split(':').next().unwrap_or(hostport)
}

#[derive(Clone, Debug)]
pub struct OnlineClient {
    /// Server as configured (`[net] server`).
    pub server: String,
    pub key: Option<String>,
    pub timeout: Duration,
}

impl OnlineClient {
    pub fn new(server: &str, key: Option<String>) -> Self {
        Self {
            server: server.trim().to_string(),
            key: key.filter(|k| !k.is_empty()),
            timeout: Duration::from_secs(8),
        }
    }

    pub fn base(&self) -> OnlineResult<String> {
        if self.server.is_empty() {
            return Err(OnlineError::NoServer);
        }
        Ok(base_url(&self.server))
    }

    /// `ws(s)://…/ws?key=` for lobby presence.
    pub fn ws_url(&self) -> OnlineResult<String> {
        let base = self.base()?;
        let ws = if let Some(r) = base.strip_prefix("https://") {
            format!("wss://{r}")
        } else if let Some(r) = base.strip_prefix("http://") {
            format!("ws://{r}")
        } else {
            base
        };
        Ok(format!("{ws}/ws?key={}", self.key()?))
    }

    fn agent(&self) -> ureq::Agent {
        self.agent_with(self.timeout)
    }

    fn agent_with(&self, timeout: Duration) -> ureq::Agent {
        ureq::Agent::config_builder()
            .timeout_global(Some(timeout))
            .http_status_as_error(false)
            .user_agent(concat!("rugen/", env!("CARGO_PKG_VERSION")))
            .build()
            .into()
    }

    fn call(&self, method: &str, path: &str, body: Option<Value>) -> OnlineResult<Value> {
        let url = format!("{}{path}", self.base()?);
        let agent = self.agent();
        let res = match (method, body) {
            ("GET", _) => agent.get(&url).header(API_HEADER, API).call(),
            ("DELETE", _) => agent.delete(&url).header(API_HEADER, API).call(),
            (_, Some(b)) => agent.post(&url).header(API_HEADER, API).send_json(&b),
            (_, None) => agent.post(&url).header(API_HEADER, API).send_empty(),
        };
        let mut resp = res.map_err(|e| OnlineError::Http(format!("{url}: {e}")))?;
        let status = resp.status().as_u16();
        let text = resp
            .body_mut()
            .read_to_string()
            .map_err(|e| OnlineError::Http(e.to_string()))?;
        let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
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

    pub fn key(&self) -> OnlineResult<&str> {
        self.key.as_deref().ok_or(OnlineError::NoKey)
    }

    fn with_key(&self, mut body: Value) -> OnlineResult<Value> {
        body["key"] = Value::String(self.key()?.to_string());
        Ok(body)
    }

    pub fn info(&self) -> OnlineResult<ServerInfo> {
        Self::decode(self.call("GET", "/", None)?)
    }

    /// Round trip of a tiny request (HTTP keep-alive aside, an upper bound).
    pub fn ping(&self) -> OnlineResult<u32> {
        let t = Instant::now();
        self.call("GET", "/health", None)?;
        Ok(t.elapsed().as_millis() as u32)
    }

    /// Create an anonymous account; the key is stored in the returned client.
    pub fn register(&mut self) -> OnlineResult<Registered> {
        let r: Registered = Self::decode(self.call("POST", "/register", Some(json!({})))?)?;
        self.key = Some(r.key.clone());
        Ok(r)
    }

    /// Create a named account (API 5 servers with [`ServerInfo::accounts`]);
    /// the new key is stored in the client. The password only travels in
    /// this request's body (HTTPS) and is never kept.
    pub fn create_account(&mut self, username: &str, password: &str) -> OnlineResult<Session> {
        let b = json!({ "username": username, "password": password });
        let s: Session = Self::decode(self.call("POST", "/v1/account/register", Some(b))?)?;
        self.key = Some(s.key.clone());
        Ok(s)
    }

    /// Log in to a named account: a new key for this device, stored in the client.
    pub fn login(&mut self, username: &str, password: &str) -> OnlineResult<Session> {
        let b = json!({ "username": username, "password": password });
        let s: Session = Self::decode(self.call("POST", "/v1/account/login", Some(b))?)?;
        self.key = Some(s.key.clone());
        Ok(s)
    }

    /// Give this (anonymous) account a username and password; it keeps its
    /// id, rating, history and key.
    pub fn claim(&self, username: &str, password: &str) -> OnlineResult<Session> {
        let b = self.with_key(json!({ "username": username, "password": password }))?;
        Self::decode(self.call("POST", "/v1/account/claim", Some(b))?)
    }

    /// Who this key belongs to (username empty: anonymous).
    pub fn account(&self) -> OnlineResult<AccountInfo> {
        Self::decode(self.call("GET", &format!("/v1/account?key={}", self.key()?), None)?)
    }

    /// Change the password: every other device is logged out, this one
    /// continues with the fresh key stored in the client.
    pub fn change_password(&mut self, old: &str, new: &str) -> OnlineResult<Session> {
        let b = self.with_key(json!({ "password": old, "new_password": new }))?;
        let s: Session = Self::decode(self.call("POST", "/v1/account/password", Some(b))?)?;
        self.key = Some(s.key.clone());
        Ok(s)
    }

    /// Revoke this device's key (`everywhere`: every key of the account).
    pub fn logout(&self, everywhere: bool) -> OnlineResult<()> {
        let path = if everywhere {
            "/v1/account/logout_all"
        } else {
            "/v1/account/logout"
        };
        self.call("POST", path, Some(self.with_key(json!({}))?))
            .map(|_| ())
    }

    pub fn set_profile(&self, name: &str, avatar: u32) -> OnlineResult<ProfileInfo> {
        let b = self.with_key(json!({ "name": name, "avatar": avatar }))?;
        Self::decode(self.call("POST", "/profile", Some(b))?)
    }

    pub fn profile(&self, id: u64) -> OnlineResult<ProfileInfo> {
        Self::decode(self.call("GET", &format!("/profile/{id}"), None)?)
    }

    /// Public rooms of this server's lobby.
    pub fn rooms(&self) -> OnlineResult<Vec<RoomSummary>> {
        Self::decode(self.call("GET", "/rooms", None)?)
    }

    pub fn create_room(&self, char_id: &str, opts: &RoomOptions) -> OnlineResult<RoomInfo> {
        let mut b = self.with_key(json!({
            "char": char_id,
            "name": opts.name,
            "public": opts.public,
            "rules": opts.rules,
            "max_spectators": opts.max_spectators,
        }))?;
        if !opts.password.is_empty() {
            b["password"] = json!(opts.password);
        }
        Self::decode(self.call("POST", "/room", Some(b))?)
    }

    /// Join as challenger (or spectator when the seat is taken / `spectate`).
    pub fn join_room(
        &self,
        code: &str,
        char_id: &str,
        password: &str,
        spectate: bool,
    ) -> OnlineResult<RoomInfo> {
        let mut b = self.with_key(json!({ "char": char_id, "spectate": spectate }))?;
        if !password.is_empty() {
            b["password"] = json!(password);
        }
        Self::decode(self.call(
            "POST",
            &format!("/room/{}/join", code.trim().to_ascii_uppercase()),
            Some(b),
        )?)
    }

    pub fn leave_room(&self, code: &str) -> OnlineResult<()> {
        let b = self.with_key(json!({}))?;
        self.call("POST", &format!("/room/{}/leave", code.trim()), Some(b))?;
        Ok(())
    }

    /// Host only: remove a member (who cannot come back).
    pub fn kick(&self, code: &str, id: u64) -> OnlineResult<RoomInfo> {
        let b = self.with_key(json!({ "id": id }))?;
        Self::decode(self.call("POST", &format!("/room/{}/kick", code.trim()), Some(b))?)
    }

    /// Ask for another match in the room (starts once both players asked).
    pub fn rematch(&self, code: &str) -> OnlineResult<RoomInfo> {
        let b = self.with_key(json!({}))?;
        Self::decode(self.call("POST", &format!("/room/{}/rematch", code.trim()), Some(b))?)
    }

    pub fn poll_room(&self, code: &str) -> OnlineResult<RoomInfo> {
        let k = self.key()?;
        Self::decode(self.call(
            "GET",
            &format!("/room/{}?key={k}", code.trim().to_ascii_uppercase()),
            None,
        )?)
    }

    /// Enter the ranked (or quick match) queue. `max_ping` = accept only
    /// opponents up to that estimated round trip; `rtt_ms` = our measured
    /// round trip to the server (used until the server measured it itself).
    pub fn enter_queue(
        &self,
        char_id: &str,
        ranked: bool,
        max_ping: Option<u32>,
        rtt_ms: Option<u32>,
    ) -> OnlineResult<QueueStatus> {
        let mut b = self.with_key(json!({ "char": char_id, "ranked": ranked }))?;
        if let Some(p) = max_ping {
            b["max_ping"] = json!(p);
        }
        if let Some(r) = rtt_ms {
            b["rtt_ms"] = json!(r);
        }
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

    /// Report the winner with the evidence the server cross-checks against
    /// the opponent's report: final frame + world checksum and both
    /// players' input stream (4 bytes per frame).
    pub fn report(
        &self,
        match_id: u64,
        winner: u64,
        evidence: &Evidence,
    ) -> OnlineResult<ResultAck> {
        let mut b = self.with_key(json!({
            "match_id": match_id,
            "winner": winner,
            "frames": evidence.frames,
        }))?;
        if let Some(c) = evidence.checksum {
            b["checksum"] = json!(c);
        }
        if !evidence.inputs.is_empty() {
            if evidence.compact {
                let rle = rugen_proto::evidence::encode_inputs(&evidence.inputs);
                b["inputs_rle"] = json!(rugen_proto::to_hex(&rle));
            } else {
                b["inputs"] = json!(rugen_proto::to_hex(&evidence.inputs));
            }
        }
        if !evidence.checksums.is_empty() {
            let c = rugen_proto::evidence::encode_checksums(&evidence.checksums);
            b["checksums"] = json!(rugen_proto::to_hex(&c));
        }
        if let Some((rx, tx)) = evidence.turn_bytes {
            b["turn"] = json!({ "rx": rx, "tx": tx });
        }
        if let Some(e) = evidence.ending {
            b["ending"] = json!(e.as_str());
        }
        Self::decode(self.call("POST", "/result", Some(b))?)
    }

    // ---- bring-your-own-character matchup

    pub fn matchup(&self, id: u64) -> OnlineResult<MatchupView> {
        let k = self.key()?;
        Self::decode(self.call("GET", &format!("/matchup/{id}?key={k}"), None)?)
    }

    /// Declare the fighter we bring (and, as P1, the stage).
    pub fn pick(
        &self,
        id: u64,
        fighter: Option<&FighterDecl>,
        stage: Option<&FighterDecl>,
    ) -> OnlineResult<MatchupView> {
        let b = self.with_key(json!({ "fighter": fighter, "stage": stage }))?;
        Self::decode(self.call("POST", &format!("/matchup/{id}/pick"), Some(b))?)
    }

    /// Accept the verdict; `unbalanced` = we agree to an unbalanced fight (casual only).
    pub fn accept(&self, id: u64, unbalanced: bool) -> OnlineResult<MatchupView> {
        let b = self.with_key(json!({ "unbalanced": unbalanced }))?;
        Self::decode(self.call("POST", &format!("/matchup/{id}/accept"), Some(b))?)
    }

    /// We hold every file of the match.
    pub fn ready(&self, id: u64, report: &ReadyReport) -> OnlineResult<MatchupView> {
        let b = self.with_key(json!({ "ready": report }))?;
        Self::decode(self.call("POST", &format!("/matchup/{id}/ready"), Some(b))?)
    }

    pub fn leave_matchup(&self, id: u64) -> OnlineResult<()> {
        let b = self.with_key(json!({}))?;
        self.call("POST", &format!("/matchup/{id}/leave"), Some(b))?;
        Ok(())
    }

    /// `HAS <fingerprint>`: stored on the server, or how much of our upload it has.
    pub fn pkg_status(&self, fp: &str) -> OnlineResult<PackageStatus> {
        let k = self.key()?;
        Self::decode(self.call("GET", &format!("/pkg/{fp}?key={k}"), None)?)
    }

    /// Upload the package file at `path` as `fp`, resuming where the server
    /// is, in [`CHUNK`]s. Returns once the server stored and verified it.
    pub fn upload_package(
        &self,
        fp: &str,
        path: &std::path::Path,
        progress: &Transfer,
    ) -> OnlineResult<PackageStatus> {
        use std::io::{Read, Seek, SeekFrom};
        let total = std::fs::metadata(path)?.len();
        progress.total.store(total, Ordering::Relaxed);
        let st = self.pkg_status(fp)?;
        if st.complete {
            progress.done.store(total, Ordering::Relaxed);
            return Ok(st);
        }
        let mut offset = st.offset.min(total);
        let mut f = std::fs::File::open(path)?;
        let key = self.key()?.to_string();
        let base = self.base()?;
        let mut buf = vec![0u8; CHUNK];
        let mut retries = 0;
        loop {
            if progress.cancel.load(Ordering::Relaxed) {
                return Err(OnlineError::Server("cancelled".into()));
            }
            progress.done.store(offset, Ordering::Relaxed);
            f.seek(SeekFrom::Start(offset))?;
            let n = (&mut f).take(CHUNK as u64).read(&mut buf)?;
            let url = format!("{base}/pkg/{fp}/upload?key={key}&offset={offset}&total={total}");
            let res = self
                .agent_with(Duration::from_secs(120))
                .post(&url)
                .header(API_HEADER, API)
                .header("Content-Type", "application/octet-stream")
                .send(&buf[..n]);
            let mut resp = match res {
                Ok(r) => r,
                Err(e) if retries < 5 => {
                    // network hiccup: ask the server where it is and resume
                    retries += 1;
                    log::info!("upload {fp}: {e}, resuming");
                    std::thread::sleep(Duration::from_millis(500 * retries));
                    let st = self.pkg_status(fp)?;
                    if st.complete {
                        return Ok(st);
                    }
                    offset = st.offset;
                    continue;
                }
                Err(e) => return Err(OnlineError::Http(format!("{url}: {e}"))),
            };
            let status = resp.status().as_u16();
            let text = resp
                .body_mut()
                .read_to_string()
                .map_err(|e| OnlineError::Http(e.to_string()))?;
            let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            match status {
                200 => {
                    let st: PackageStatus = Self::decode(v)?;
                    if st.complete {
                        progress.done.store(total, Ordering::Relaxed);
                        return Ok(st);
                    }
                    if st.offset <= offset && n > 0 {
                        return Err(OnlineError::Server("upload made no progress".into()));
                    }
                    offset = st.offset;
                    retries = 0;
                }
                409 if v.get("offset").is_some() => {
                    offset = v["offset"].as_u64().unwrap_or(0);
                }
                _ => {
                    let msg = v
                        .get("error")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("http {status}"));
                    return Err(OnlineError::Server(msg));
                }
            }
        }
    }

    /// Download package `fp` (`size` bytes) into `dest`, resuming a partial
    /// file left there by an interrupted download.
    pub fn download_package(
        &self,
        fp: &str,
        size: u64,
        dest: &std::path::Path,
        progress: &Transfer,
    ) -> OnlineResult<()> {
        use std::io::Write;
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        progress.total.store(size, Ordering::Relaxed);
        let mut have = std::fs::metadata(dest).map_or(0, |m| m.len());
        if have > size {
            std::fs::remove_file(dest)?;
            have = 0;
        }
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dest)?;
        let key = self.key()?.to_string();
        let base = self.base()?;
        let mut retries = 0;
        while have < size {
            if progress.cancel.load(Ordering::Relaxed) {
                return Err(OnlineError::Server("cancelled".into()));
            }
            progress.done.store(have, Ordering::Relaxed);
            let url = format!("{base}/pkg/{fp}/data?key={key}&offset={have}&len={CHUNK}");
            let res = self
                .agent_with(Duration::from_secs(120))
                .get(&url)
                .header(API_HEADER, API)
                .call();
            let mut resp = match res {
                Ok(r) => r,
                Err(e) if retries < 5 => {
                    retries += 1;
                    log::info!("download {fp}: {e}, resuming");
                    std::thread::sleep(Duration::from_millis(500 * retries));
                    continue;
                }
                Err(e) => return Err(OnlineError::Http(format!("{url}: {e}"))),
            };
            let status = resp.status().as_u16();
            let bytes = resp
                .body_mut()
                .with_config()
                .limit(8 << 20)
                .read_to_vec()
                .map_err(|e| OnlineError::Http(e.to_string()))?;
            if status != 200 {
                let v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                let msg = v
                    .get("error")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("http {status}"));
                return Err(OnlineError::Server(msg));
            }
            if bytes.is_empty() {
                return Err(OnlineError::Server("package shorter than announced".into()));
            }
            f.write_all(&bytes)?;
            have += bytes.len() as u64;
            retries = 0;
        }
        f.flush()?;
        progress.done.store(size, Ordering::Relaxed);
        Ok(())
    }

    /// Tell the server this match desynchronised (it will not resolve by agreement).
    pub fn report_desync(&self, match_id: u64, frame: u32) -> OnlineResult<()> {
        let b = self.with_key(json!({ "frame": frame }))?;
        self.call("POST", &format!("/match/{match_id}/desync"), Some(b))?;
        Ok(())
    }

    pub fn char_rating(&self, fp: &str) -> OnlineResult<Option<CharRating>> {
        Self::decode(self.call("GET", &format!("/char/{fp}"), None)?)
    }

    pub fn replay(&self, match_id: u64) -> OnlineResult<Replay> {
        Self::decode(self.call("GET", &format!("/replay/{match_id}"), None)?)
    }

    pub fn leaderboard(&self) -> OnlineResult<Leaderboard> {
        self.leaderboard_of(None)
    }

    /// The board of a finished season (`Some(n)`, its final standings) or
    /// the live one (`None`). Older servers ignore `season` and answer the
    /// live board (`finished` false).
    pub fn leaderboard_of(&self, season: Option<u32>) -> OnlineResult<Leaderboard> {
        self.board_query(season, None)
    }

    /// A per-fighter board of `season` (`None`: the running one): of
    /// `fighter` (a fingerprint), or of the most played fighter when
    /// `None`; the answer lists the fighters with a board. Older servers
    /// answer the players' board (no `fighters`).
    pub fn fighter_board(
        &self,
        season: Option<u32>,
        fighter: Option<&str>,
    ) -> OnlineResult<Leaderboard> {
        self.board_query(season, Some(fighter.unwrap_or("")))
    }

    fn board_query(&self, season: Option<u32>, fighter: Option<&str>) -> OnlineResult<Leaderboard> {
        let mut q: Vec<String> = Vec::new();
        match fighter {
            Some("") => q.push("fighters=1".into()),
            Some(fp) => q.push(format!("fighter={fp}")),
            None => {}
        }
        if let Some(k) = &self.key {
            q.push(format!("key={k}"));
        }
        if let Some(n) = season {
            q.push(format!("season={n}"));
        }
        let path = if q.is_empty() {
            "/leaderboard".to_string()
        } else {
            format!("/leaderboard?{}", q.join("&"))
        };
        Self::decode(self.call("GET", &path, None)?)
    }

    pub fn season(&self) -> OnlineResult<Season> {
        Self::decode(self.call("GET", "/seasons/current", None)?)
    }

    // ---- official server / signatures (API 4)

    /// The server's own signed truth feed (`GET /v1/truth`).
    pub fn truth(&self) -> OnlineResult<SignedBlob> {
        Self::decode(self.call("GET", "/v1/truth", None)?)
    }

    /// The official feed a community server holds (`GET /v1/truth/official`).
    pub fn official_mirror(&self) -> OnlineResult<SignedBlob> {
        Self::decode(self.call("GET", "/v1/truth/official", None)?)
    }

    /// The server signs our random `nonce` (proves which key it holds).
    pub fn identity(&self, nonce: &str) -> OnlineResult<SignedBlob> {
        Self::decode(self.call("GET", &format!("/v1/identity?nonce={nonce}"), None)?)
    }

    /// Outcome of one of our matches, signed once settled.
    pub fn match_result(&self, match_id: u64) -> OnlineResult<ResultAck> {
        let k = self.key()?;
        Self::decode(self.call("GET", &format!("/match/{match_id}/result?key={k}"), None)?)
    }
}

/// Lobby WebSocket on a background thread: receives presence pushes (room
/// changes, queue matches, kicks) and answers the server's pings, which is
/// how the server measures our round trip for matchmaking.
pub struct Presence {
    rx: Receiver<rugen_proto::ServerMsg>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Presence {
    pub fn spawn(client: &OnlineClient) -> OnlineResult<Self> {
        use std::sync::atomic::{AtomicBool, Ordering};
        let url = client.ws_url()?;
        let (tx, rx) = channel();
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        std::thread::spawn(move || {
            let mut backoff = Duration::from_secs(1);
            while !flag.load(Ordering::Relaxed) {
                match tungstenite::connect(url.as_str()) {
                    Ok((mut ws, _)) => {
                        backoff = Duration::from_secs(1);
                        let tcp = match ws.get_ref() {
                            tungstenite::stream::MaybeTlsStream::Plain(t) => Some(t),
                            tungstenite::stream::MaybeTlsStream::Rustls(t) => Some(t.get_ref()),
                            _ => None,
                        };
                        if let Some(t) = tcp {
                            let _ = t.set_read_timeout(Some(Duration::from_millis(250)));
                        }
                        let mut last_ping = Instant::now();
                        while !flag.load(Ordering::Relaxed) {
                            if last_ping.elapsed() > Duration::from_secs(30) {
                                let ping = serde_json::to_string(&rugen_proto::ClientMsg::Ping)
                                    .unwrap_or_default();
                                if ws.send(tungstenite::Message::Text(ping.into())).is_err() {
                                    break;
                                }
                                last_ping = Instant::now();
                            }
                            match ws.read() {
                                Ok(tungstenite::Message::Text(t)) => {
                                    if let Ok(m) = serde_json::from_str(&t)
                                        && tx.send(m).is_err()
                                    {
                                        return;
                                    }
                                }
                                Ok(tungstenite::Message::Close(_)) => break,
                                Ok(_) => {}
                                Err(tungstenite::Error::Io(e))
                                    if matches!(
                                        e.kind(),
                                        std::io::ErrorKind::WouldBlock
                                            | std::io::ErrorKind::TimedOut
                                    ) =>
                                {
                                    // flush queued pongs
                                    let _ = ws.flush();
                                }
                                Err(_) => break,
                            }
                        }
                        let _ = ws.close(None);
                    }
                    Err(e) => log::debug!("presence: {e}"),
                }
                std::thread::sleep(backoff);
                backoff = (backoff * 2).min(Duration::from_secs(30));
            }
        });
        Ok(Self { rx, stop })
    }

    /// Pushes received since the last call.
    pub fn drain(&mut self) -> Vec<rugen_proto::ServerMsg> {
        self.rx.try_iter().collect()
    }
}

impl Drop for Presence {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Options of a new room.
#[derive(Clone, Debug, Default)]
pub struct RoomOptions {
    pub name: String,
    pub public: bool,
    /// Empty = no password.
    pub password: String,
    pub rules: Rules,
    pub max_spectators: u8,
}

/// What a result report carries for verification.
#[derive(Clone, Debug, Default)]
pub struct Evidence {
    pub frames: u32,
    pub checksum: Option<u64>,
    pub inputs: Vec<u8>,
    /// World checksums every 60 frames ([`rugen_proto::evidence`]).
    pub checksums: Vec<(u32, u64)>,
    /// Upload the inputs run-length encoded (`inputs_rle`); only servers
    /// that issue TURN transports understand it.
    pub compact: bool,
    /// Bytes received / sent through TURN (wire estimate), if used.
    pub turn_bytes: Option<(u64, u64)>,
    /// The match ended early: our forfeit or the other player's exit.
    pub ending: Option<rugen_proto::Ending>,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_urls() {
        assert_eq!(
            base_url("https://rugen.example.com/"),
            "https://rugen.example.com"
        );
        assert_eq!(base_url("1.2.3.4:7777"), "http://1.2.3.4:7777");
        assert_eq!(url_host("https://rugen.example.com/x"), "rugen.example.com");
        assert_eq!(url_host("http://1.2.3.4:7777"), "1.2.3.4");
        assert_eq!(url_host("http://[::1]:80"), "::1");
        let c = OnlineClient::new("https://r.example.com", Some("k".into()));
        assert_eq!(c.ws_url().unwrap(), "wss://r.example.com/ws?key=k");
        let c = OnlineClient::new("127.0.0.1:8080", Some("k".into()));
        assert_eq!(c.ws_url().unwrap(), "ws://127.0.0.1:8080/ws?key=k");
        assert!(matches!(
            OnlineClient::new("", None).info(),
            Err(OnlineError::NoServer)
        ));
    }
}
