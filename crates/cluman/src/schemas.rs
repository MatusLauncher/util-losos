use std::{
    collections::HashMap,
    net::Ipv4Addr,
    str::FromStr,
    sync::{Arc, Mutex},
};

use either::Either;
use miette::miette;
use serde::{Deserialize, Serialize};
use strum::{EnumIter, IntoEnumIterator};

// ── Mode ─────────────────────────────────────────────────────────────────────

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
    /// files and tells servers what to run.
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

// ── Registration payload ─────────────────────────────────────────────────────

/// Describes the network membership of a node.
///
/// * `ips = Either::Right((addr, mode))` — a single-peer registration request
///   sent by a node that wants to join the cluster.
/// * `ips = Either::Left(map)` — the full registry kept internally by a
///   Controller or Server.
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
    /// If `self.ips` is currently `Right`, it is first converted into a `Left`
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

    /// Borrow the `ips` field.
    pub fn ips(&self) -> &Either<HashMap<Ipv4Addr, Mode>, (Ipv4Addr, Mode)> {
        &self.ips
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

// ── Task list ─────────────────────────────────────────────────────────────────

/// A list of Docker Compose task identifiers (file paths or compose project
/// names) that the Controller wants executed.
#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Tasks {
    tasks: Vec<String>,
}

#[allow(dead_code)]
impl Tasks {
    pub fn new(tasks: Vec<String>) -> Self {
        Self { tasks }
    }

    pub fn tasks(&self) -> &[String] {
        &self.tasks
    }

    /// Append a task (compose file path or project name) to the list.
    pub fn add_task(&mut self, compose_file: String) {
        self.tasks.push(compose_file);
    }

    /// Remove and return the next pending task, if any.
    pub fn pop_task(&mut self) -> Option<String> {
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

/// Shared state owned by a **Controller** node.
///
/// Cloning is cheap — all fields are `Arc`-wrapped.
#[derive(Clone, Default)]
pub struct ControllerState {
    /// Servers that have registered with this controller.
    pub servers: Arc<Mutex<HashMap<Ipv4Addr, Mode>>>,
    /// Tasks waiting to be dispatched to servers.
    pub tasks: Arc<Mutex<Tasks>>,
}

impl ControllerState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_server(&self, ip: Ipv4Addr) {
        self.servers.lock().unwrap().insert(ip, Mode::Server);
    }

    pub fn add_task(&self, task: String) {
        self.tasks.lock().unwrap().add_task(task);
    }

    /// Snapshot the current task list for serialisation (e.g. `GET /tasks`).
    pub fn snapshot_tasks(&self) -> Tasks {
        self.tasks.lock().unwrap().clone()
    }

    /// Snapshot the registered server addresses.
    pub fn server_addrs(&self) -> Vec<Ipv4Addr> {
        self.servers.lock().unwrap().keys().copied().collect()
    }
}

/// Shared state owned by a **Server** node.
///
/// Cloning is cheap — all fields are `Arc`-wrapped.
#[derive(Clone, Default)]
pub struct ServerState {
    /// Clients that have registered with this server.
    pub clients: Arc<Mutex<HashMap<Ipv4Addr, Mode>>>,
    /// Tasks received from the controller(s), not yet claimed by a client.
    pub pending_tasks: Arc<Mutex<Tasks>>,
}

impl ServerState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_client(&self, ip: Ipv4Addr) {
        self.clients.lock().unwrap().insert(ip, Mode::Client);
    }

    /// Enqueue tasks received from a controller poll.
    pub fn enqueue_tasks(&self, incoming: Tasks) {
        let mut lock = self.pending_tasks.lock().unwrap();
        for t in incoming.tasks() {
            lock.add_task(t.clone());
        }
    }

    /// Claim the next pending task for a client to execute.
    pub fn claim_task(&self) -> Option<String> {
        self.pending_tasks.lock().unwrap().pop_task()
    }

    pub fn client_addrs(&self) -> Vec<Ipv4Addr> {
        self.clients.lock().unwrap().keys().copied().collect()
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
