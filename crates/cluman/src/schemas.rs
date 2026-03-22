use std::{
    collections::HashMap,
    net::Ipv4Addr,
    str::FromStr,
    sync::{Arc, Mutex},
};

use either::Either;
use ipnet::Ipv4Net;
use miette::miette;
use serde::{Deserialize, Serialize};
use strum::{EnumIter, IntoEnumIterator};

// ── IpRange ───────────────────────────────────────────────────────────────────

/// An IPv4 address range accepted in three notations:
///
/// | Notation     | Example                  | Meaning                            |
/// |--------------|--------------------------|------------------------------------|
/// | Single       | `10.0.0.1`               | Exactly one host                   |
/// | CIDR         | `10.0.0.0/24`            | All host addresses in the subnet   |
/// | Dash range   | `10.0.0.1-10.0.0.20`     | All addresses from start to end    |
///
/// Use [`IpRange::hosts`] to expand any variant into a flat `Vec<Ipv4Addr>`.
#[derive(Debug, Clone)]
pub enum IpRange {
    Single(Ipv4Addr),
    Cidr(Ipv4Net),
    DashRange(Ipv4Addr, Ipv4Addr),
}

impl IpRange {
    /// Expand into an ordered list of host addresses.
    ///
    /// * `Single` → one-element vec.
    /// * `Cidr`   → every host address in the subnet (network and broadcast
    ///               addresses are excluded, matching [`Ipv4Net::hosts`]).
    /// * `DashRange` → every address from `start` to `end`, inclusive.
    pub fn hosts(&self) -> Vec<Ipv4Addr> {
        match self {
            IpRange::Single(addr) => vec![*addr],
            IpRange::Cidr(net) => net.hosts().collect(),
            IpRange::DashRange(start, end) => {
                let start_n = u32::from(*start);
                let end_n = u32::from(*end);
                (start_n..=end_n).map(Ipv4Addr::from).collect()
            }
        }
    }
}

impl FromStr for IpRange {
    type Err = miette::Error;

    /// Parse an `IpRange` from a string.
    ///
    /// Precedence:
    /// 1. Contains `/`  → attempt CIDR parse via [`Ipv4Net`].
    /// 2. Contains `-`  → split on the *first* `-` and parse two [`Ipv4Addr`]s.
    /// 3. Otherwise     → parse a single [`Ipv4Addr`].
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.contains('/') {
            return s
                .parse::<Ipv4Net>()
                .map(IpRange::Cidr)
                .map_err(|e| miette!("Invalid CIDR range '{}': {}", s, e));
        }

        if let Some((start_s, end_s)) = s.split_once('-') {
            let start = start_s
                .parse::<Ipv4Addr>()
                .map_err(|e| miette!("Invalid start address '{}': {}", start_s, e))?;
            let end = end_s
                .parse::<Ipv4Addr>()
                .map_err(|e| miette!("Invalid end address '{}': {}", end_s, e))?;
            if u32::from(start) > u32::from(end) {
                return Err(miette!(
                    "Range start '{}' must not be greater than end '{}'",
                    start,
                    end
                ));
            }
            return Ok(IpRange::DashRange(start, end));
        }

        s.parse::<Ipv4Addr>()
            .map(IpRange::Single)
            .map_err(|e| miette!("Invalid IP address or range '{}': {}", s, e))
    }
}

// ── Mode ──────────────────────────────────────────────────────────────────────

#[derive(
    Serialize, Deserialize, Default, EnumIter, Clone, Copy, PartialEq, Eq, PartialOrd, Ord,
)]
pub enum Mode {
    #[default]
    /// Does the tasks.
    Client,
    /// Runs on MDL and assigns tasks to [`Self::Client`]s.
    Server,
    /// Usually a dev workstation / CI container that owns the Docker Compose
    /// files and pushes them directly to servers.
    Controller,
}

impl ToString for Mode {
    fn to_string(&self) -> String {
        match self {
            Mode::Client => "client".into(),
            Mode::Server => "server".into(),
            Mode::Controller => "controller".into(),
        }
    }
}

impl FromStr for Mode {
    type Err = miette::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "client" => Ok(Self::Client),
            "server" => Ok(Self::Server),
            "controller" | "cluman" => Ok(Self::Controller),
            other => Err(miette!(
                "Invalid operation mode '{}'. Expected one of: {}",
                other,
                Mode::iter()
                    .map(|m| m.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }
}

// ── Registration payload ──────────────────────────────────────────────────────

/// Describes the network membership of a node.
///
/// * `ips = Either::Right((addr, mode))` — a single-peer registration request
///   sent by a node that wants to join the cluster.
/// * `ips = Either::Left(map)` — the full registry kept internally by a Server.
#[derive(Serialize, Deserialize, Clone)]
pub struct CluManSchema {
    pub mode: Mode,
    pub ips: Either<HashMap<Ipv4Addr, Mode>, (Ipv4Addr, Mode)>,
}

impl Default for CluManSchema {
    fn default() -> Self {
        Self {
            mode: Mode::default(),
            ips: Either::Left(HashMap::new()),
        }
    }
}

#[allow(dead_code)]
impl CluManSchema {
    /// Create a registration request (the `Right` variant) for a single peer.
    pub fn registration(ip: Ipv4Addr, mode: Mode) -> Self {
        Self {
            mode,
            ips: Either::Right((ip, mode)),
        }
    }

    /// Insert `(ip, mode)` into the internal registry map.
    ///
    /// If `self.ips` is currently `Right`, it is first promoted into a `Left`
    /// map containing that single entry before the new one is added.
    pub fn add(&mut self, ip: Ipv4Addr, mode: Mode) {
        let map = match &self.ips {
            Either::Left(m) => {
                let mut m = m.clone();
                m.insert(ip, mode);
                m
            }
            Either::Right((existing_ip, existing_mode)) => {
                let mut m = HashMap::new();
                m.insert(*existing_ip, *existing_mode);
                m.insert(ip, mode);
                m
            }
        };
        self.ips = Either::Left(map);
    }

    /// Return the peer described by the `Right` variant, if present.
    pub fn peer(&self) -> Option<(Ipv4Addr, Mode)> {
        self.ips.as_ref().right().copied()
    }

    /// Return the full registry map, if present.
    pub fn registry(&self) -> Option<&HashMap<Ipv4Addr, Mode>> {
        self.ips.as_ref().left()
    }
}

// ── Task ──────────────────────────────────────────────────────────────────────

/// A single Docker Compose task pushed by a Controller.
///
/// Carrying the file *content* (rather than a path) means the task is fully
/// self-contained and can be forwarded from Server to Client without any shared
/// filesystem.
#[derive(Serialize, Deserialize, Clone)]
pub struct Task {
    /// Original filename, e.g. `"docker-compose.yml"`.  Used for logging and
    /// as the temp-file name on the client.
    pub filename: String,
    /// Full UTF-8 content of the compose file.
    pub content: String,
}

impl Task {
    pub fn new(filename: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            filename: filename.into(),
            content: content.into(),
        }
    }
}

// ── Task queue ────────────────────────────────────────────────────────────────

/// An ordered queue of [`Task`]s waiting to be claimed by a client.
#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Tasks {
    tasks: Vec<Task>,
}

#[allow(dead_code)]
impl Tasks {
    pub fn new(tasks: Vec<Task>) -> Self {
        Self { tasks }
    }

    pub fn tasks(&self) -> &[Task] {
        &self.tasks
    }

    /// Append a task to the back of the queue.
    pub fn push(&mut self, task: Task) {
        self.tasks.push(task);
    }

    /// Remove and return the next pending task (FIFO), if any.
    pub fn pop(&mut self) -> Option<Task> {
        if self.tasks.is_empty() {
            None
        } else {
            Some(self.tasks.remove(0))
        }
    }

    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    pub fn len(&self) -> usize {
        self.tasks.len()
    }
}

// ── Shared state types ────────────────────────────────────────────────────────

// Note: Controllers are one-shot processes — they push files directly to
// servers and exit. No ControllerState is needed.

/// Shared state owned by a **Server** node.
///
/// Cloning is cheap — all fields are `Arc`-wrapped.
#[derive(Clone, Default)]
pub struct ServerState {
    /// Clients that have registered with this server.
    pub clients: Arc<Mutex<HashMap<Ipv4Addr, Mode>>>,
    /// Tasks pushed by controllers, not yet claimed by a client.
    pub pending_tasks: Arc<Mutex<Tasks>>,
}

impl ServerState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_client(&self, ip: Ipv4Addr) {
        self.clients.lock().unwrap().insert(ip, Mode::Client);
    }

    /// Enqueue a task received from a controller push.
    pub fn push_task(&self, task: Task) {
        self.pending_tasks.lock().unwrap().push(task);
    }

    /// Claim the next pending task for a client to execute.
    pub fn claim_task(&self) -> Option<Task> {
        self.pending_tasks.lock().unwrap().pop()
    }

    /// Snapshot the registered client addresses.
    pub fn client_addrs(&self) -> Vec<Ipv4Addr> {
        self.clients.lock().unwrap().keys().copied().collect()
    }

    /// Number of tasks currently waiting in the queue.
    pub fn pending_count(&self) -> usize {
        self.pending_tasks.lock().unwrap().len()
    }
}

/// State carried by a **Client** node.
#[derive(Clone)]
pub struct ClientState {
    /// Base URL of the server this client is registered with.
    pub server_url: String,
    /// This client's own IPv4 address (used in the registration payload).
    #[allow(dead_code)]
    pub own_ip: Ipv4Addr,
}

impl ClientState {
    pub fn new(server_url: String, own_ip: Ipv4Addr) -> Self {
        Self { server_url, own_ip }
    }
}
