//! HTTP daemon and matching thin client for the user database.
//!
//! # Server — [`Daemon`]
//!
//! An async expressjs HTTP server that persists all user data as a JSON file
//! (`Users { uschemas: Vec<UserSchema> }`).  It listens on **port 20** and
//! exposes five REST routes:
//!
//! | Method   | Path                   | Description            |
//! |----------|------------------------|------------------------|
//! | `GET`    | `/healthcheck`         | Liveness probe         |
//! | `GET`    | `/user/get/:name`      | Fetch a single user    |
//! | `GET`    | `/users`               | Fetch all users        |
//! | `POST`   | `/user/create`         | Create a user          |
//! | `DELETE` | `/user/delete/:name`   | Delete a user          |
//! | `PATCH`  | `/user/update/:name`   | Update a user field    |
//!
//! Every route enforces [`Daemon::validate_location`] so that a local daemon
//! refuses non-loopback requests and a remote daemon refuses loopback ones.
//!
//! # Client — [`UserAPI`]
//!
//! A thin synchronous [`ureq`] wrapper used by the CLI and by `perman`.
//! Defaults to `127.0.0.1`; call [`UserAPI::set_addr`] to point it at a
//! remote daemon.

use std::{
    fs::{read_to_string, write},
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    path::PathBuf,
};

use base64::{Engine, prelude::BASE64_STANDARD};
use expressjs::prelude::*;
use miette::{IntoDiagnostic, miette};
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::mode::Location;

/// The HTTP daemon that owns the user database.
///
/// Construct with [`Daemon::new`] and start with [`Daemon::run`].
/// The database is stored at `save_location` as a JSON-serialised [`Users`].
#[derive(Default, Clone)]
pub struct Daemon {
    save_location: PathBuf,
    users: Users,
    location: Location,
}

/// Top-level container for the JSON user database.
#[derive(Default, Clone, Serialize, Deserialize)]
pub struct Users {
    uschemas: Vec<UserSchema>,
}

/// All persistent data for a single user account.
///
/// Passwords are stored **base64-encoded** (not hashed).  2FA material is
/// stored in the corresponding `totp_*` / `fido2_*` / `second_pass` fields
/// depending on the active [`TwoFA`] method.
#[derive(Default, Clone, Serialize, Deserialize, Debug)]
pub struct UserSchema {
    user: String,
    /// Base64-encoded primary password.
    pass: String,
    /// Directories the user is allowed to `chdir` into (enforced by perman).
    allowed_dirs: Vec<PathBuf>,
    locked_out: bool,
    /// Whether the home directory uses LUKS2 encryption.
    encryption: bool,
    twofa: Option<TwoFA>,
    #[serde(default)]
    totp_secret: Option<Vec<u8>>,
    #[serde(default)]
    fido2_credential_id: Option<Vec<u8>>,
    #[serde(default)]
    fido2_public_key_der: Option<Vec<u8>>,
    /// Key-type tag: `0` = Unknown, `1` = Ecdsa256, `2` = Ed25519.
    #[serde(default)]
    fido2_public_key_type: Option<u8>,
    #[serde(default)]
    second_pass: Option<String>,
    #[serde(default)]
    luks_device: Option<PathBuf>,
}

/// Active second-factor method for a user.
#[derive(Clone, Serialize, Deserialize, Debug)]
pub enum TwoFA {
    /// FIDO2 / CTAP HID hardware authenticator.
    Passkey,
    /// Secondary plain-text password.
    Password,
    /// RFC 6238 time-based one-time password.
    TOTP,
}
impl UserSchema {
    /// Set the username. Returns `self` to allow method chaining.
    pub fn set_user(&mut self, user: String) -> Self {
        self.user = user;
        self.clone()
    }

    /// Base64-encode `pass` and store it. Returns `self` for chaining.
    pub fn set_pass(&mut self, pass: String) -> Self {
        self.pass = BASE64_STANDARD.encode(pass);
        self.clone()
    }

    /// Replace the list of directories the user is permitted to enter.
    pub fn set_allowed_dirs(&mut self, allowed_dirs: Vec<PathBuf>) -> UserSchema {
        self.allowed_dirs = allowed_dirs;
        self.clone()
    }

    /// Return `true` if the account is locked out.
    pub fn locked_out(&self) -> bool {
        self.locked_out
    }

    /// Enable or disable LUKS2 home-directory encryption for this user.
    pub fn set_encryption(&mut self, encryption: bool) -> UserSchema {
        self.encryption = encryption;
        self.clone()
    }

    /// Set or clear the active 2FA method. Pass `None` to disable 2FA.
    pub fn set_twofa(&mut self, twofa: Option<TwoFA>) -> UserSchema {
        self.twofa = twofa;
        self.clone()
    }

    /// Store a raw TOTP secret (20 bytes produced by [`generate_totp_secret`]).
    ///
    /// [`generate_totp_secret`]: crate::twofa::generate_totp_secret
    pub fn set_totp_secret(&mut self, secret: Vec<u8>) -> UserSchema {
        self.totp_secret = Some(secret);
        self.clone()
    }

    /// Store the FIDO2 credential returned by [`register_fido2`].
    ///
    /// [`register_fido2`]: crate::twofa::register_fido2
    pub fn set_fido2_credential(
        &mut self,
        credential_id: Vec<u8>,
        public_key_der: Vec<u8>,
        public_key_type: u8,
    ) -> UserSchema {
        self.fido2_credential_id = Some(credential_id);
        self.fido2_public_key_der = Some(public_key_der);
        self.fido2_public_key_type = Some(public_key_type);
        self.clone()
    }

    /// Set the secondary password used with [`TwoFA::Password`].
    pub fn set_second_pass(&mut self, pass: String) -> UserSchema {
        self.second_pass = Some(pass);
        self.clone()
    }

    /// Record the LUKS2 block device path for this user's encrypted home.
    pub fn set_luks_device(&mut self, device: PathBuf) -> UserSchema {
        self.luks_device = Some(device);
        self.clone()
    }

    /// Return the base64-encoded password string as stored on disk.
    pub fn pass(&self) -> &str {
        &self.pass
    }

    /// Return the username.
    pub fn name(&self) -> &str {
        &self.user
    }

    /// Return `true` if the home directory uses LUKS2 encryption.
    pub fn encryption(&self) -> bool {
        self.encryption
    }

    /// Return the active 2FA method, or `None` if 2FA is disabled.
    pub fn twofa(&self) -> Option<&TwoFA> {
        self.twofa.as_ref()
    }

    /// Return the raw TOTP secret bytes, if present.
    pub fn totp_secret(&self) -> Option<&[u8]> {
        self.totp_secret.as_deref()
    }

    /// Return the FIDO2 credential ID, if present.
    pub fn fido2_credential_id(&self) -> Option<&[u8]> {
        self.fido2_credential_id.as_deref()
    }

    /// Return the DER-encoded FIDO2 public key, if present.
    pub fn fido2_public_key_der(&self) -> Option<&[u8]> {
        self.fido2_public_key_der.as_deref()
    }

    /// Return the key-type tag (`0` Unknown, `1` Ecdsa256, `2` Ed25519), if present.
    pub fn fido2_public_key_type(&self) -> Option<u8> {
        self.fido2_public_key_type
    }

    /// Return the secondary password used with [`TwoFA::Password`], if set.
    pub fn second_pass(&self) -> Option<&str> {
        self.second_pass.as_deref()
    }

    /// Return the path to the LUKS2 block device, if configured.
    pub fn luks_device(&self) -> Option<&std::path::Path> {
        self.luks_device.as_deref()
    }

    /// Return the list of directories the user is allowed to enter.
    pub fn allowed_dirs(&self) -> &[PathBuf] {
        &self.allowed_dirs
    }
}

/// Wire type for `PATCH /user/update/:name` requests.
#[derive(Serialize, Deserialize)]
pub struct ChangeSchema {
    what: What,
}

/// Discriminated union describing which field of a [`UserSchema`] to change.
///
/// One `PATCH` request carries exactly one `What` variant.
#[derive(Serialize, Deserialize)]
pub enum What {
    /// Lock or unlock the account.
    LockoutStatus(bool),
    /// Replace the primary password (should already be base64-encoded).
    Password(String),
    /// Replace the list of allowed directories.
    AllowedDirectories(Vec<PathBuf>),
    /// Set or clear the active 2FA method.
    TwoFactor(Option<TwoFA>),
    /// Store a new raw TOTP secret.
    TOTPSecret(Vec<u8>),
    /// Store a new FIDO2 credential.
    FIDOCredential {
        credential_id: Vec<u8>,
        public_key_der: Vec<u8>,
        /// Key-type tag: `0` Unknown, `1` Ecdsa256, `2` Ed25519.
        public_key_type: u8,
    },
    /// Set the secondary password used with [`TwoFA::Password`].
    SecondPassword(String),
    /// Set the LUKS2 block device path.
    LuksDevice(PathBuf),
}

impl Daemon {
    /// Create a new daemon that will accept connections matching `location`.
    pub fn new(location: Location) -> Self {
        Self {
            location,
            ..Default::default()
        }
    }
    /// Read the database from disk and return the schema for `user`.
    fn get(&self, user: String) -> miette::Result<UserSchema> {
        let f = read_to_string(&self.save_location).into_diagnostic()?;
        let users: Users = serde_json::from_str(&f).into_diagnostic()?;
        users
            .uschemas
            .into_iter()
            .find(|u| u.user == user)
            .ok_or_else(|| miette!("User '{}' not found", user))
    }
    /// Append `schema` to the database and persist it.
    /// If the file does not yet exist the database starts empty.
    fn create(&self, schema: UserSchema) -> miette::Result<()> {
        let mut users: Users = read_to_string(&self.save_location)
            .ok()
            .and_then(|f| serde_json::from_str(&f).ok())
            .unwrap_or_default();
        users.uschemas.push(schema);
        write(
            &self.save_location,
            serde_json::to_string(&users).into_diagnostic()?,
        )
        .into_diagnostic()
    }
    /// Remove the entry for `user` from the database.
    /// Returns an error if no such user exists.
    fn delete(&self, user: String) -> miette::Result<()> {
        let mut users: Users = read_to_string(&self.save_location)
            .ok()
            .and_then(|f| serde_json::from_str(&f).ok())
            .unwrap_or_default();
        let before = users.uschemas.len();
        users.uschemas.retain(|u| u.user != user);
        // If the length is unchanged no entry matched — the user did not exist.
        if users.uschemas.len() == before {
            return Err(miette!("User '{}' not found", user));
        }
        write(
            &self.save_location,
            serde_json::to_string(&users).into_diagnostic()?,
        )
        .into_diagnostic()
    }
    /// Apply `change` to the stored schema for `user` and persist the result.
    fn update(&self, user: String, change: What) -> miette::Result<()> {
        let mut users: Users = read_to_string(&self.save_location)
            .ok()
            .and_then(|f| serde_json::from_str(&f).ok())
            .unwrap_or_default();
        let schema = users
            .uschemas
            .iter_mut()
            .find(|u| u.user == user)
            .ok_or_else(|| miette!("User '{}' not found", user))?;
        match change {
            What::LockoutStatus(v) => schema.locked_out = v,
            What::Password(v) => schema.pass = v,
            What::AllowedDirectories(v) => schema.allowed_dirs = v,
            What::TwoFactor(v) => schema.twofa = v,
            What::TOTPSecret(v) => schema.totp_secret = Some(v),
            What::FIDOCredential {
                credential_id,
                public_key_der,
                public_key_type,
            } => {
                schema.fido2_credential_id = Some(credential_id);
                schema.fido2_public_key_der = Some(public_key_der);
                schema.fido2_public_key_type = Some(public_key_type);
            }
            What::SecondPassword(v) => schema.second_pass = Some(v),
            What::LuksDevice(v) => schema.luks_device = Some(v),
        }
        write(
            &self.save_location,
            serde_json::to_string(&users).into_diagnostic()?,
        )
        .into_diagnostic()
    }
    /// Enforce the network-origin policy for this daemon instance.
    ///
    /// A `Local` daemon only accepts loopback addresses (`127.0.0.1` / `::1`).
    /// A `Remote` daemon only accepts non-loopback addresses.
    fn validate_location(&self, ip: IpAddr) -> miette::Result<()> {
        if (ip == Ipv6Addr::LOCALHOST || ip == Ipv4Addr::LOCALHOST)
            && self.location == Location::Local
            || (ip != Ipv6Addr::LOCALHOST || ip != Ipv4Addr::LOCALHOST)
                && self.location == Location::Remote
        {
            Ok(())
        } else {
            Err(miette!("Improperly configured user handling."))
        }
    }
    /// Register all HTTP routes and start listening on port 20.
    ///
    /// Each route clones the daemon handle into the async closure and calls
    /// [`validate_location`] before processing the request.
    ///
    /// [`validate_location`]: Daemon::validate_location
    pub async fn run(&self) -> miette::Result<()> {
        let mut app = express();

        app.get("/healthcheck", async move |_, res: Response| {
            res.status_code(200)
        });

        let daemon = self.clone();
        app.get("/user/get/:name", move |req: Request, res: Response| {
            let daemon = daemon.clone();
            async move {
                let ip = req
                    .ip()
                    .map(|s| s.ip())
                    .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
                if daemon.validate_location(ip).is_ok() {
                    let name = req
                        .params()
                        .get("name")
                        .map(|s| s.to_owned())
                        .unwrap_or_default();
                    match daemon.get(name) {
                        Ok(schema) => res.send_json(&schema),
                        Err(_) => Response::not_found(),
                    }
                } else {
                    res.status_code(403)
                }
            }
        });

        let daemon = self.clone();
        app.get("/users", move |req: Request, res: Response| {
            let daemon = daemon.clone();
            async move {
                let ip = req
                    .ip()
                    .map(|s| s.ip())
                    .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
                if daemon.validate_location(ip).is_ok() {
                    match read_to_string(&daemon.save_location)
                        .ok()
                        .and_then(|f| serde_json::from_str::<Users>(&f).ok())
                    {
                        Some(users) => res.send_json(&users.uschemas),
                        None => Response::internal_server_error(),
                    }
                } else {
                    res.status_code(403)
                }
            }
        });

        let daemon = self.clone();
        app.post("/user/create", move |req: Request, res: Response| {
            let daemon = daemon.clone();
            async move {
                let ip = req
                    .ip()
                    .map(|s| s.ip())
                    .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
                if daemon.validate_location(ip).is_ok() {
                    match req.json::<UserSchema>().await {
                        Ok(schema) => match daemon.create(schema) {
                            Ok(()) => res,
                            Err(_) => Response::internal_server_error(),
                        },
                        Err(_) => res.status_code(400).send_text("No schema supplied"),
                    }
                } else {
                    res.status_code(403)
                }
            }
        });

        let daemon = self.clone();
        app.delete("/user/delete/:name", move |req: Request, res: Response| {
            let daemon = daemon.clone();
            async move {
                let ip = req
                    .ip()
                    .map(|s| s.ip())
                    .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
                if daemon.validate_location(ip).is_ok() {
                    let name = req
                        .params()
                        .get("name")
                        .map(|s| s.to_owned())
                        .unwrap_or_default();
                    match daemon.delete(name) {
                        Ok(()) => res,
                        Err(_) => Response::not_found(),
                    }
                } else {
                    res.status_code(403)
                }
            }
        });

        let daemon = self.clone();
        app.patch("/user/update/:name", move |req: Request, res: Response| {
            let daemon = daemon.clone();
            async move {
                let ip = req
                    .ip()
                    .map(|s| s.ip())
                    .unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST));
                if daemon.validate_location(ip).is_ok() {
                    let name = req
                        .params()
                        .get("name")
                        .map(|s| s.to_owned())
                        .unwrap_or_default();
                    match req.json::<ChangeSchema>().await {
                        Ok(cs) => match daemon.update(name, cs.what) {
                            Ok(()) => res,
                            Err(_) => Response::not_found(),
                        },
                        Err(_) => res.status_code(400).send_text("No change schema supplied"),
                    }
                } else {
                    res.status_code(403)
                }
            }
        });

        app.use_global(LoggingMiddleware);
        app.listen(20, |_| async {}).await; // this'll be running as a system daemon, after all.
        Ok(())
    }
}

/// Synchronous HTTP client for the userman daemon.
///
/// Defaults to `127.0.0.1`; use [`UserAPI::set_addr`] to target a remote
/// instance.  All methods return a `miette::Result`; network or HTTP errors
/// are wrapped via `IntoDiagnostic`.
pub struct UserAPI {
    addr: IpAddr,
}

impl Default for UserAPI {
    fn default() -> Self {
        Self::new()
    }
}

impl UserAPI {
    /// Create a client pointing at `127.0.0.1:20`.
    pub fn new() -> Self {
        Self {
            addr: Ipv4Addr::LOCALHOST.into(),
        }
    }
    /// Fetch all user schemas from the daemon.
    pub fn users(&self) -> miette::Result<Vec<UserSchema>> {
        let response = ureq::get(&format!("http://{}:20/users", self.addr))
            .call()
            .into_diagnostic()?;
        Ok(response.into_json().into_diagnostic()?)
    }
    /// Fetch the schema for a single user by name.
    pub fn user(&self, name: &str) -> miette::Result<UserSchema> {
        let response = ureq::get(&format!("http://{}:20/user/get/{name}", self.addr))
            .call()
            .into_diagnostic()?;
        Ok(response.into_json().into_diagnostic()?)
    }
    /// Send a create request for the given `schema`.
    pub fn create_user(&self, schema: &UserSchema) -> miette::Result<()> {
        info!("Creating user from schema {:#?}", schema);
        ureq::post(&format!(
            "http://{}:20/user/create/{}",
            self.addr, schema.user
        ))
        .send_json(schema)
        .into_diagnostic()?;
        Ok(())
    }
    /// Delete the user with the given `name`.
    pub fn delete_user(&self, name: &str) -> miette::Result<()> {
        info!("Deleting user {name}");
        ureq::delete(&format!("http://{}:20/user/delete/{name}", self.addr))
            .call()
            .into_diagnostic()?;
        Ok(())
    }
    /// Apply a single field change to the named user.
    pub fn update_user(&self, name: &str, change: What) -> miette::Result<()> {
        ureq::patch(&format!("http://{}:20/user/update/{name}", self.addr))
            .send_json(&ChangeSchema { what: change })
            .into_diagnostic()?;
        Ok(())
    }

    /// Override the target daemon address (default: `127.0.0.1`).
    pub fn set_addr(&mut self, addr: IpAddr) {
        self.addr = addr;
    }
}
