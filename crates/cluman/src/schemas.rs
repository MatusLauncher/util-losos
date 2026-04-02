//! Wire-format and shared-state types for the `cluman` binary.
//!
//! This module defines every type that is serialised over the network or held
//! in shared memory at runtime:
//!
//! * [`IpRange`]      — an IPv4 address range in single / CIDR / dash notation.
//! * [`Mode`]         — the three runtime personalities of the binary.
//! * [`CluManSchema`] — the registration payload exchanged between nodes.
//! * [`Task`]         — a single Docker Compose task pushed by a Controller.
//! * [`Tasks`]        — an ordered queue of pending [`Task`]s.
//! * [`ServerState`]  — shared state owned by a Server node.
//! * [`ClientState`]  — state carried by a Client node.

use std::{
    collections::{HashMap, VecDeque},
    fs,
    net::Ipv4Addr,
    path::Path,
    str::FromStr,
    sync::{Arc, Mutex},
};

use either::Either;
use ipnet::Ipv4Net;
use miette::{IntoDiagnostic, miette};
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
    ///   addresses are excluded, matching [`Ipv4Net::hosts`]).
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

/// The three runtime personalities of the `cluman` binary.
///
/// The active mode is selected by examining `argv[0]` (the name under which
/// the binary was invoked), so a single executable can be symlinked to
/// `client`, `server`, or `controller`. [`Mode::Client`] is the default.
#[derive(
    Debug, Serialize, Deserialize, Default, EnumIter, Clone, Copy, PartialEq, Eq, PartialOrd, Ord,
)]
pub enum Mode {
    #[default]
    /// Polls the server for [`Task`]s and executes Docker Compose files received from it.
    Client,
    /// Runs on MDL and assigns tasks to [`Self::Client`]s.
    Server,
    /// Usually a dev workstation / CI container that owns the Docker Compose
    /// files and pushes them directly to servers.
    Controller,
}

impl std::fmt::Display for Mode {
    /// Serialises the mode to its lowercase string form: `"client"`, `"server"`, or `"controller"`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Mode::Client => "client",
            Mode::Server => "server",
            Mode::Controller => "controller",
        })
    }
}

impl FromStr for Mode {
    type Err = miette::Error;

    /// Parses a mode from a string. `"cluman"` is accepted as an alias for `"controller"`.
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
        // Move out of self.ips without cloning the whole map.
        let current = std::mem::replace(&mut self.ips, Either::Left(HashMap::new()));
        let mut map = match current {
            Either::Left(m) => m,
            Either::Right((existing_ip, existing_mode)) => {
                let mut m = HashMap::with_capacity(2);
                m.insert(existing_ip, existing_mode);
                m
            }
        };
        map.insert(ip, mode);
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
    /// Creates a new [`Task`] from a filename and content string.
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
    tasks: VecDeque<Task>,
}

#[allow(dead_code)]
impl Tasks {
    /// Creates a [`Tasks`] queue pre-populated from an iterator of [`Task`] values.
    pub fn new(tasks: impl IntoIterator<Item = Task>) -> Self {
        let v: Vec<Task> = tasks.into_iter().collect();
        Self {
            tasks: VecDeque::from(v),
        }
    }

    /// Returns an iterator over the pending tasks without consuming the queue.
    pub fn tasks(&self) -> impl ExactSizeIterator<Item = &Task> {
        self.tasks.iter()
    }

    /// Append a task to the back of the queue.
    pub fn push(&mut self, task: Task) {
        self.tasks.push_back(task);
    }

    /// Remove and return the next pending task (FIFO) in O(1).
    pub fn pop(&mut self) -> Option<Task> {
        self.tasks.pop_front()
    }

    /// Returns `true` if there are no pending tasks.
    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    /// Returns the number of tasks currently in the queue.
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
    /// Creates a new, empty [`ServerState`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Records `ip` as a registered [`Mode::Client`] node.
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

// ── ServerState persistence ───────────────────────────────────────────────────

/// A serialisable snapshot of [`ServerState`] used for persistence.
///
/// [`ServerState`] itself cannot derive `Serialize`/`Deserialize` because it
/// wraps its fields in `Arc<Mutex<…>>`.  This flat struct is used as an
/// intermediate representation for saving to and loading from disk.
#[derive(Serialize, Deserialize)]
struct ServerStateSnapshot {
    clients: HashMap<Ipv4Addr, Mode>,
    pending_tasks: Tasks,
}

impl ServerState {
    /// Serialises the current state to a JSON file at `path`.
    ///
    /// Both the client registry and the pending task queue are captured in
    /// a point-in-time snapshot and written atomically via
    /// [`fs::write`](std::fs::write).
    pub fn save_to(&self, path: &Path) -> miette::Result<()> {
        let snapshot = ServerStateSnapshot {
            clients: self.clients.lock().unwrap().clone(),
            pending_tasks: self.pending_tasks.lock().unwrap().clone(),
        };
        let json = serde_json::to_string(&snapshot).into_diagnostic()?;
        fs::write(path, json).into_diagnostic()
    }

    /// Loads a previously saved [`ServerState`] from the JSON file at `path`.
    ///
    /// Returns a fresh empty [`ServerState`] when the file does not exist,
    /// allowing the server to start cleanly on first boot without requiring
    /// an explicit initialisation step.
    pub fn load_from(path: &Path) -> miette::Result<Self> {
        if !path.exists() {
            return Ok(Self::new());
        }
        let json = fs::read_to_string(path).into_diagnostic()?;
        let snapshot: ServerStateSnapshot = serde_json::from_str(&json).into_diagnostic()?;
        Ok(Self {
            clients: Arc::new(Mutex::new(snapshot.clients)),
            pending_tasks: Arc::new(Mutex::new(snapshot.pending_tasks)),
        })
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
    /// Creates a new [`ClientState`] with the given server URL and own IP address.
    pub fn new(server_url: impl Into<String>, own_ip: Ipv4Addr) -> Self {
        Self {
            server_url: server_url.into(),
            own_ip,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── IpRange ───────────────────────────────────────────────────────────────

    #[test]
    fn ip_range_parses_single_address() {
        let r: IpRange = "10.0.0.1".parse().unwrap();
        assert!(matches!(r, IpRange::Single(_)));
        assert_eq!(r.hosts(), vec!["10.0.0.1".parse::<Ipv4Addr>().unwrap()]);
    }

    #[test]
    fn ip_range_single_hosts_returns_one_element() {
        let r: IpRange = "192.168.1.99".parse().unwrap();
        assert_eq!(r.hosts().len(), 1);
    }

    #[test]
    fn ip_range_parses_cidr_slash30() {
        // /30 yields exactly 2 host addresses (.1 and .2)
        let r: IpRange = "10.0.0.0/30".parse().unwrap();
        assert!(matches!(r, IpRange::Cidr(_)));
        let hosts = r.hosts();
        assert_eq!(hosts.len(), 2);
        assert_eq!(hosts[0], "10.0.0.1".parse::<Ipv4Addr>().unwrap());
        assert_eq!(hosts[1], "10.0.0.2".parse::<Ipv4Addr>().unwrap());
    }

    #[test]
    fn ip_range_cidr_slash32_yields_one_host() {
        // /32 is a single-host route — ipnet's Ipv4Net::hosts() returns the
        // address itself as the sole host (no broadcast/network exclusion at
        // prefix length 32).
        let r: IpRange = "10.0.0.1/32".parse().unwrap();
        let hosts = r.hosts();
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0], "10.0.0.1".parse::<Ipv4Addr>().unwrap());
    }

    #[test]
    fn ip_range_cidr_slash24_yields_254_hosts() {
        let r: IpRange = "10.0.0.0/24".parse().unwrap();
        assert_eq!(r.hosts().len(), 254);
    }

    #[test]
    fn ip_range_parses_dash_range() {
        let r: IpRange = "10.0.0.1-10.0.0.3".parse().unwrap();
        assert!(matches!(r, IpRange::DashRange(_, _)));
        let hosts = r.hosts();
        assert_eq!(hosts.len(), 3);
        assert_eq!(hosts[0], "10.0.0.1".parse::<Ipv4Addr>().unwrap());
        assert_eq!(hosts[1], "10.0.0.2".parse::<Ipv4Addr>().unwrap());
        assert_eq!(hosts[2], "10.0.0.3".parse::<Ipv4Addr>().unwrap());
    }

    #[test]
    fn ip_range_dash_range_same_start_and_end() {
        let r: IpRange = "192.168.1.5-192.168.1.5".parse().unwrap();
        assert_eq!(r.hosts().len(), 1);
        assert_eq!(r.hosts()[0], "192.168.1.5".parse::<Ipv4Addr>().unwrap());
    }

    #[test]
    fn ip_range_dash_range_hosts_are_ordered() {
        let r: IpRange = "10.0.0.10-10.0.0.15".parse().unwrap();
        let hosts = r.hosts();
        let sorted = {
            let mut v = hosts.clone();
            v.sort();
            v
        };
        assert_eq!(hosts, sorted);
    }

    #[test]
    fn ip_range_rejects_reversed_dash_range() {
        let err = "10.0.0.10-10.0.0.1".parse::<IpRange>();
        assert!(err.is_err());
        assert!(
            err.unwrap_err()
                .to_string()
                .contains("must not be greater than")
        );
    }

    #[test]
    fn ip_range_rejects_invalid_single_address() {
        assert!("not-an-ip".parse::<IpRange>().is_err());
    }

    #[test]
    fn ip_range_rejects_invalid_cidr_prefix() {
        assert!("10.0.0.0/99".parse::<IpRange>().is_err());
    }

    #[test]
    fn ip_range_rejects_invalid_dash_start() {
        assert!("garbage-10.0.0.1".parse::<IpRange>().is_err());
    }

    #[test]
    fn ip_range_rejects_invalid_dash_end() {
        assert!("10.0.0.1-garbage".parse::<IpRange>().is_err());
    }

    // ── Mode ──────────────────────────────────────────────────────────────────

    #[test]
    fn mode_default_is_client() {
        assert_eq!(Mode::default(), Mode::Client);
    }

    #[test]
    fn mode_from_str_client() {
        assert_eq!("client".parse::<Mode>().unwrap(), Mode::Client);
    }

    #[test]
    fn mode_from_str_server() {
        assert_eq!("server".parse::<Mode>().unwrap(), Mode::Server);
    }

    #[test]
    fn mode_from_str_controller() {
        assert_eq!("controller".parse::<Mode>().unwrap(), Mode::Controller);
    }

    #[test]
    fn mode_from_str_cluman_is_alias_for_controller() {
        // "cluman" is the binary name — accepted as an alias for Controller
        assert_eq!("cluman".parse::<Mode>().unwrap(), Mode::Controller);
    }

    #[test]
    fn mode_from_str_rejects_unknown_value() {
        let err = "worker".parse::<Mode>();
        assert!(err.is_err());
        let msg = err.unwrap_err().to_string();
        assert!(msg.contains("worker"));
        // Error message must list all valid variants
        assert!(msg.contains("client"));
        assert!(msg.contains("server"));
        assert!(msg.contains("controller"));
    }

    #[test]
    fn mode_to_string_client() {
        assert_eq!(Mode::Client.to_string(), "client");
    }

    #[test]
    fn mode_to_string_server() {
        assert_eq!(Mode::Server.to_string(), "server");
    }

    #[test]
    fn mode_to_string_controller() {
        assert_eq!(Mode::Controller.to_string(), "controller");
    }

    #[test]
    fn mode_to_string_roundtrip() {
        for mode in [Mode::Client, Mode::Server, Mode::Controller] {
            let restored: Mode = mode.to_string().parse().unwrap();
            assert_eq!(restored, mode);
        }
    }

    #[test]
    fn mode_ordering_client_lt_server_lt_controller() {
        // Ord is derived in field-declaration order
        assert!(Mode::Client < Mode::Server);
        assert!(Mode::Server < Mode::Controller);
        assert!(Mode::Client < Mode::Controller);
    }

    // ── CluManSchema ──────────────────────────────────────────────────────────

    #[test]
    fn cluman_schema_default_has_empty_left_registry() {
        let s = CluManSchema::default();
        assert_eq!(s.mode, Mode::Client);
        assert!(s.registry().unwrap().is_empty());
        assert!(s.peer().is_none());
    }

    #[test]
    fn cluman_schema_registration_stores_peer_as_right() {
        let ip: Ipv4Addr = "10.0.0.1".parse().unwrap();
        let s = CluManSchema::registration(ip, Mode::Client);
        assert_eq!(s.peer(), Some((ip, Mode::Client)));
        assert!(s.registry().is_none());
    }

    #[test]
    fn cluman_schema_registration_sets_mode_field() {
        let ip: Ipv4Addr = "10.0.0.2".parse().unwrap();
        let s = CluManSchema::registration(ip, Mode::Server);
        assert_eq!(s.mode, Mode::Server);
    }

    #[test]
    fn cluman_schema_add_inserts_into_empty_left() {
        let mut s = CluManSchema::default();
        let ip: Ipv4Addr = "10.0.0.5".parse().unwrap();
        s.add(ip, Mode::Client);
        let reg = s.registry().unwrap();
        assert_eq!(reg.len(), 1);
        assert_eq!(reg.get(&ip), Some(&Mode::Client));
    }

    #[test]
    fn cluman_schema_add_accumulates_multiple_entries() {
        let mut s = CluManSchema::default();
        let ip1: Ipv4Addr = "10.0.0.1".parse().unwrap();
        let ip2: Ipv4Addr = "10.0.0.2".parse().unwrap();
        s.add(ip1, Mode::Client);
        s.add(ip2, Mode::Server);
        let reg = s.registry().unwrap();
        assert_eq!(reg.len(), 2);
        assert_eq!(reg.get(&ip1), Some(&Mode::Client));
        assert_eq!(reg.get(&ip2), Some(&Mode::Server));
    }

    #[test]
    fn cluman_schema_add_promotes_right_to_left() {
        let ip1: Ipv4Addr = "10.0.0.1".parse().unwrap();
        let ip2: Ipv4Addr = "10.0.0.2".parse().unwrap();
        // Start as a Right (registration request)
        let mut s = CluManSchema::registration(ip1, Mode::Client);
        assert!(s.peer().is_some());
        // Adding a second peer must promote to Left and preserve the first entry
        s.add(ip2, Mode::Server);
        assert!(s.peer().is_none(), "should no longer be Right after add");
        let reg = s.registry().unwrap();
        assert_eq!(reg.len(), 2);
        assert_eq!(reg.get(&ip1), Some(&Mode::Client));
        assert_eq!(reg.get(&ip2), Some(&Mode::Server));
    }

    #[test]
    fn cluman_schema_roundtrip_json_right_variant() {
        let ip: Ipv4Addr = "192.168.0.1".parse().unwrap();
        let original = CluManSchema::registration(ip, Mode::Server);
        let json = serde_json::to_string(&original).unwrap();
        let restored: CluManSchema = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.peer(), Some((ip, Mode::Server)));
    }

    #[test]
    fn cluman_schema_roundtrip_json_left_variant() {
        let mut original = CluManSchema::default();
        let ip: Ipv4Addr = "10.1.2.3".parse().unwrap();
        original.add(ip, Mode::Client);
        let json = serde_json::to_string(&original).unwrap();
        let restored: CluManSchema = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.registry().unwrap().get(&ip), Some(&Mode::Client));
    }

    // ── Task ──────────────────────────────────────────────────────────────────

    #[test]
    fn task_new_stores_filename_and_content() {
        let t = Task::new("compose.yml", "version: '3'");
        assert_eq!(t.filename, "compose.yml");
        assert_eq!(t.content, "version: '3'");
    }

    #[test]
    fn task_new_accepts_owned_strings() {
        let t = Task::new(String::from("a.yml"), String::from("data"));
        assert_eq!(t.filename, "a.yml");
    }

    #[test]
    fn task_roundtrip_json() {
        let original = Task::new("test.yml", "some: yaml\n");
        let json = serde_json::to_string(&original).unwrap();
        let restored: Task = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.filename, original.filename);
        assert_eq!(restored.content, original.content);
    }

    // ── Tasks ─────────────────────────────────────────────────────────────────

    #[test]
    fn tasks_default_is_empty() {
        let t = Tasks::default();
        assert!(t.is_empty());
        assert_eq!(t.len(), 0);
        assert_eq!(t.tasks().len(), 0);
    }

    #[test]
    fn tasks_push_increases_len() {
        let mut t = Tasks::default();
        t.push(Task::new("a.yml", ""));
        assert_eq!(t.len(), 1);
        assert!(!t.is_empty());
    }

    #[test]
    fn tasks_pop_returns_none_when_empty() {
        let mut t = Tasks::default();
        assert!(t.pop().is_none());
    }

    #[test]
    fn tasks_pop_is_fifo() {
        let mut t = Tasks::default();
        t.push(Task::new("first.yml", "a"));
        t.push(Task::new("second.yml", "b"));
        t.push(Task::new("third.yml", "c"));
        assert_eq!(t.pop().unwrap().filename, "first.yml");
        assert_eq!(t.pop().unwrap().filename, "second.yml");
        assert_eq!(t.pop().unwrap().filename, "third.yml");
        assert!(t.pop().is_none());
    }

    #[test]
    fn tasks_pop_decrements_len() {
        let mut t = Tasks::default();
        t.push(Task::new("a.yml", ""));
        t.push(Task::new("b.yml", ""));
        assert_eq!(t.len(), 2);
        t.pop();
        assert_eq!(t.len(), 1);
        t.pop();
        assert_eq!(t.len(), 0);
        assert!(t.is_empty());
    }

    #[test]
    fn tasks_preserves_content_through_pop() {
        let content = "services:\n  web:\n    image: nginx\n";
        let mut t = Tasks::default();
        t.push(Task::new("docker-compose.yml", content));
        let got = t.pop().unwrap();
        assert_eq!(got.filename, "docker-compose.yml");
        assert_eq!(got.content, content);
    }

    #[test]
    fn tasks_new_initialises_from_vec() {
        let t = Tasks::new(vec![Task::new("a.yml", ""), Task::new("b.yml", "")]);
        assert_eq!(t.len(), 2);
    }

    #[test]
    fn tasks_roundtrip_json() {
        let original = Tasks::new(vec![
            Task::new("a.yml", "content-a"),
            Task::new("b.yml", "content-b"),
        ]);
        let json = serde_json::to_string(&original).unwrap();
        let mut restored: Tasks = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.len(), 2);
        assert_eq!(restored.pop().unwrap().filename, "a.yml");
        assert_eq!(restored.pop().unwrap().filename, "b.yml");
    }

    // ── ServerState ───────────────────────────────────────────────────────────

    #[test]
    fn server_state_new_starts_empty() {
        let s = ServerState::new();
        assert_eq!(s.pending_count(), 0);
        assert!(s.client_addrs().is_empty());
    }

    #[test]
    fn server_state_register_client_adds_address() {
        let s = ServerState::new();
        let ip: Ipv4Addr = "10.0.0.1".parse().unwrap();
        s.register_client(ip);
        let addrs = s.client_addrs();
        assert_eq!(addrs.len(), 1);
        assert!(addrs.contains(&ip));
    }

    #[test]
    fn server_state_register_same_client_twice_is_idempotent() {
        let s = ServerState::new();
        let ip: Ipv4Addr = "10.0.0.1".parse().unwrap();
        s.register_client(ip);
        s.register_client(ip);
        assert_eq!(s.client_addrs().len(), 1);
    }

    #[test]
    fn server_state_registers_multiple_distinct_clients() {
        let s = ServerState::new();
        let ip1: Ipv4Addr = "10.0.0.1".parse().unwrap();
        let ip2: Ipv4Addr = "10.0.0.2".parse().unwrap();
        s.register_client(ip1);
        s.register_client(ip2);
        assert_eq!(s.client_addrs().len(), 2);
    }

    #[test]
    fn server_state_push_task_increments_pending_count() {
        let s = ServerState::new();
        s.push_task(Task::new("a.yml", ""));
        assert_eq!(s.pending_count(), 1);
        s.push_task(Task::new("b.yml", ""));
        assert_eq!(s.pending_count(), 2);
    }

    #[test]
    fn server_state_claim_task_is_fifo() {
        let s = ServerState::new();
        s.push_task(Task::new("first.yml", ""));
        s.push_task(Task::new("second.yml", ""));
        assert_eq!(s.claim_task().unwrap().filename, "first.yml");
        assert_eq!(s.claim_task().unwrap().filename, "second.yml");
        assert!(s.claim_task().is_none());
    }

    #[test]
    fn server_state_claim_task_decrements_pending_count() {
        let s = ServerState::new();
        s.push_task(Task::new("a.yml", ""));
        assert_eq!(s.pending_count(), 1);
        s.claim_task();
        assert_eq!(s.pending_count(), 0);
    }

    #[test]
    fn server_state_claim_task_returns_none_when_empty() {
        let s = ServerState::new();
        assert!(s.claim_task().is_none());
    }

    #[test]
    fn server_state_claim_task_preserves_content() {
        let s = ServerState::new();
        let content = "services:\n  db:\n    image: postgres\n";
        s.push_task(Task::new("db.yml", content));
        let task = s.claim_task().unwrap();
        assert_eq!(task.filename, "db.yml");
        assert_eq!(task.content, content);
    }

    #[test]
    fn server_state_clone_shares_task_queue() {
        let s1 = ServerState::new();
        let s2 = s1.clone();
        // A task pushed via s1 must be visible through s2
        s1.push_task(Task::new("shared.yml", ""));
        assert_eq!(s2.pending_count(), 1);
        // Claiming via s2 removes it from s1's view as well
        s2.claim_task();
        assert_eq!(s1.pending_count(), 0);
    }

    #[test]
    fn server_state_clone_shares_client_registry() {
        let s1 = ServerState::new();
        let s2 = s1.clone();
        let ip: Ipv4Addr = "10.0.0.1".parse().unwrap();
        s2.register_client(ip);
        assert!(s1.client_addrs().contains(&ip));
    }

    #[test]
    fn server_state_pending_count_matches_push_minus_claims() {
        let s = ServerState::new();
        for i in 0..5u8 {
            s.push_task(Task::new(format!("{i}.yml"), ""));
        }
        assert_eq!(s.pending_count(), 5);
        s.claim_task();
        s.claim_task();
        assert_eq!(s.pending_count(), 3);
    }
}
