//! Per-connection SSH handler implementing [`russh::server::Handler`].

use std::borrow::Cow;
use std::fmt;
use std::os::fd::AsRawFd;

use async_trait::async_trait;
use russh::server::{Auth, Msg, Session};
use russh::{Channel, ChannelId, CryptoVec, MethodSet};
use ssh_key::PublicKey;
use tokio::io::AsyncReadExt;
use tracing::{info, warn};
use userman::daemon::{TwoFA, UserAPI, UserSchema};

use crate::auth as sshauth;
use crate::session::PtySession;

// ---------------------------------------------------------------------------
// Custom error type that satisfies `From<russh::Error> + Send`
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum SshError {
    Russh(russh::Error),
    Miette(String),
}

impl fmt::Display for SshError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SshError::Russh(e) => write!(f, "SSH error: {e}"),
            SshError::Miette(s) => write!(f, "{s}"),
        }
    }
}

impl std::error::Error for SshError {}

impl From<russh::Error> for SshError {
    fn from(e: russh::Error) -> Self {
        SshError::Russh(e)
    }
}

impl From<miette::Error> for SshError {
    fn from(e: miette::Error) -> Self {
        SshError::Miette(e.to_string())
    }
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

/// Per-connection state.
pub struct SshHandler {
    user_api: UserAPI,
    /// Set after successful primary authentication.
    authenticated_user: Option<UserSchema>,
    /// True when password/pubkey passed but 2FA is still pending.
    primary_auth_done: bool,
    /// Active PTY session (created on pty_request + shell_request).
    pty: Option<PtySession>,
    /// Requested terminal type.
    term: String,
    /// Terminal dimensions.
    col_width: u32,
    row_height: u32,
}

impl SshHandler {
    pub fn new(user_api: UserAPI) -> Self {
        Self {
            user_api,
            authenticated_user: None,
            primary_auth_done: false,
            pty: None,
            term: "xterm".to_string(),
            col_width: 80,
            row_height: 24,
        }
    }

    /// Decide what to do after primary auth succeeds: accept immediately or
    /// push to keyboard-interactive for 2FA.
    fn post_primary_auth(&mut self, user: UserSchema) -> Auth {
        match user.twofa() {
            None => {
                info!("User '{}' authenticated (no 2FA)", user.name());
                self.authenticated_user = Some(user);
                Auth::Accept
            }
            Some(TwoFA::Passkey) => {
                warn!(
                    "User '{}' has Passkey 2FA which is not supported over SSH",
                    user.name()
                );
                Auth::Reject {
                    proceed_with_methods: None,
                }
            }
            Some(TwoFA::TOTP | TwoFA::Password) => {
                info!(
                    "User '{}' primary auth OK — proceeding to keyboard-interactive for 2FA",
                    user.name()
                );
                self.authenticated_user = Some(user);
                self.primary_auth_done = true;
                Auth::Reject {
                    proceed_with_methods: Some(MethodSet::KEYBOARD_INTERACTIVE),
                }
            }
        }
    }
}

#[async_trait]
impl russh::server::Handler for SshHandler {
    type Error = SshError;

    async fn auth_password(
        &mut self,
        username: &str,
        password: &str,
    ) -> Result<Auth, Self::Error> {
        info!("Password auth attempt for user '{username}'");

        let user = match self.user_api.user(username) {
            Ok(u) => u,
            Err(_) => {
                warn!("User '{username}' not found");
                return Ok(Auth::Reject {
                    proceed_with_methods: None,
                });
            }
        };

        if let Err(reason) = sshauth::check_account(&user) {
            warn!("Auth rejected for '{username}': {reason}");
            return Ok(Auth::Reject {
                proceed_with_methods: None,
            });
        }

        if !sshauth::verify_password(&user, password) {
            warn!("Invalid password for '{username}'");
            return Ok(Auth::Reject {
                proceed_with_methods: None,
            });
        }

        Ok(self.post_primary_auth(user))
    }

    async fn auth_publickey(
        &mut self,
        username: &str,
        public_key: &PublicKey,
    ) -> Result<Auth, Self::Error> {
        info!("Public key auth attempt for user '{username}'");

        let user = match self.user_api.user(username) {
            Ok(u) => u,
            Err(_) => {
                warn!("User '{username}' not found");
                return Ok(Auth::Reject {
                    proceed_with_methods: None,
                });
            }
        };

        if let Err(reason) = sshauth::check_account(&user) {
            warn!("Auth rejected for '{username}': {reason}");
            return Ok(Auth::Reject {
                proceed_with_methods: None,
            });
        }

        if !sshauth::match_public_key(&user, public_key) {
            info!("No matching public key for '{username}'");
            return Ok(Auth::Reject {
                proceed_with_methods: None,
            });
        }

        Ok(self.post_primary_auth(user))
    }

    async fn auth_keyboard_interactive(
        &mut self,
        username: &str,
        _submethods: &str,
        response: Option<russh::server::Response<'async_trait>>,
    ) -> Result<Auth, Self::Error> {
        let user = match &self.authenticated_user {
            Some(u) => u,
            None => {
                warn!("Keyboard-interactive without prior auth for '{username}'");
                return Ok(Auth::Reject {
                    proceed_with_methods: None,
                });
            }
        };

        // First call (no response yet) — send the appropriate prompt.
        let Some(response) = response else {
            let (prompt, echo) = match user.twofa() {
                Some(TwoFA::TOTP) => ("TOTP code: ", true),
                Some(TwoFA::Password) => ("Second password: ", false),
                _ => {
                    return Ok(Auth::Reject {
                        proceed_with_methods: None,
                    });
                }
            };
            return Ok(Auth::Partial {
                name: Cow::Borrowed("Two-Factor Authentication"),
                instructions: Cow::Borrowed(""),
                prompts: Cow::Owned(vec![(Cow::Borrowed(prompt), echo)]),
            });
        };

        // Second call — validate the response.
        let answer: String = response
            .flat_map(|b| String::from_utf8(b.to_vec()).ok())
            .next()
            .unwrap_or_default();

        let ok = match user.twofa() {
            Some(TwoFA::TOTP) => sshauth::verify_totp(user, &answer)?,
            Some(TwoFA::Password) => sshauth::verify_second_password(user, &answer),
            _ => false,
        };

        if ok {
            info!("2FA passed for user '{username}'");
            Ok(Auth::Accept)
        } else {
            warn!("2FA failed for user '{username}'");
            self.authenticated_user = None;
            self.primary_auth_done = false;
            Ok(Auth::Reject {
                proceed_with_methods: None,
            })
        }
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        _session: &mut Session,
    ) -> Result<bool, Self::Error> {
        info!("Session channel opened (id {:?})", channel.id());
        Ok(true)
    }

    async fn pty_request(
        &mut self,
        channel: ChannelId,
        term: &str,
        col_width: u32,
        row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        _modes: &[(russh::Pty, u32)],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        info!("PTY request: term={term} cols={col_width} rows={row_height}");
        self.term = term.to_string();
        self.col_width = col_width;
        self.row_height = row_height;
        session.channel_success(channel)?;
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel_id: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let user = match &self.authenticated_user {
            Some(u) => u.clone(),
            None => {
                warn!("Shell request without authentication");
                session.close(channel_id)?;
                return Ok(());
            }
        };

        info!("Shell request for user '{}'", user.name());

        let pty_session = PtySession::spawn_shell(
            &user,
            &self.term,
            self.col_width,
            self.row_height,
        )?;

        let master_raw_fd = pty_session.master_fd.as_raw_fd();
        let child_pid = pty_session.child_pid;
        self.pty = Some(pty_session);

        session.channel_success(channel_id)?;

        // Spawn async task to relay PTY output → SSH channel.
        let handle = session.handle();
        tokio::spawn(async move {
            relay_pty_to_channel(master_raw_fd, child_pid, channel_id, handle).await;
        });

        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel_id: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let user = match &self.authenticated_user {
            Some(u) => u.clone(),
            None => {
                warn!("Exec request without authentication");
                session.close(channel_id)?;
                return Ok(());
            }
        };

        let command = String::from_utf8_lossy(data);
        info!("Exec request for user '{}': {command}", user.name());

        let pty_session = PtySession::spawn_exec(
            &user,
            &command,
            &self.term,
            self.col_width,
            self.row_height,
        )?;

        let master_raw_fd = pty_session.master_fd.as_raw_fd();
        let child_pid = pty_session.child_pid;
        self.pty = Some(pty_session);

        session.channel_success(channel_id)?;

        let handle = session.handle();
        tokio::spawn(async move {
            relay_pty_to_channel(master_raw_fd, child_pid, channel_id, handle).await;
        });

        Ok(())
    }

    async fn data(
        &mut self,
        _channel_id: ChannelId,
        data: &[u8],
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        // Write client input to the PTY master.
        if let Some(pty) = &self.pty {
            let fd = pty.master_fd.as_raw_fd();
            let data = data.to_vec();
            tokio::task::spawn_blocking(move || {
                unsafe {
                    libc::write(fd, data.as_ptr() as *const libc::c_void, data.len());
                }
            });
        }
        Ok(())
    }

    async fn window_change_request(
        &mut self,
        _channel_id: ChannelId,
        col_width: u32,
        row_height: u32,
        _pix_width: u32,
        _pix_height: u32,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        if let Some(pty) = &self.pty {
            pty.resize(col_width, row_height);
        }
        self.col_width = col_width;
        self.row_height = row_height;
        Ok(())
    }

    async fn channel_eof(
        &mut self,
        channel_id: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        info!("Channel EOF on {channel_id:?}");
        if let Some(pty) = &self.pty {
            pty.kill();
        }
        session.close(channel_id)?;
        Ok(())
    }
}

impl Drop for SshHandler {
    fn drop(&mut self) {
        if let Some(pty) = &self.pty {
            pty.kill();
            pty.try_wait();
        }
    }
}

/// Read from the PTY master fd and send data to the SSH channel.
/// Exits when the child process terminates or the fd closes.
async fn relay_pty_to_channel(
    master_raw_fd: i32,
    child_pid: i32,
    channel_id: ChannelId,
    handle: russh::server::Handle,
) {
    // Duplicate the fd so the relay task owns its own copy.
    let dup_fd = unsafe { libc::dup(master_raw_fd) };
    if dup_fd < 0 {
        warn!("Failed to dup PTY master fd");
        return;
    }

    let async_file = match unsafe { crate::session::master_to_async(dup_fd) } {
        Ok(f) => f,
        Err(e) => {
            warn!("Failed to create async reader for PTY: {e}");
            return;
        }
    };

    let mut reader = tokio::io::BufReader::new(async_file);
    let mut buf = [0u8; 4096];

    loop {
        match reader.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => {
                let data = CryptoVec::from_slice(&buf[..n]);
                if handle.data(channel_id, data).await.is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }

    // Wait for child exit and send exit status.
    let mut status = 0i32;
    unsafe { libc::waitpid(child_pid, &mut status, 0) };
    let exit_code = if libc::WIFEXITED(status) {
        libc::WEXITSTATUS(status) as u32
    } else {
        1
    };

    let _ = handle.exit_status_request(channel_id, exit_code).await;
    let _ = handle.eof(channel_id).await;
    let _ = handle.close(channel_id).await;
}
