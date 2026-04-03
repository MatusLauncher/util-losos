//! SSH server setup and listener.

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use miette::{IntoDiagnostic, miette};
use russh::server::Server as _;
use ssh_key::PrivateKey;
use tracing::info;
use userman::daemon::UserAPI;

use crate::handler::SshHandler;
use crate::hostkey;

/// Configuration for the SSH server.
pub struct SshConfig {
    pub host_key: PrivateKey,
    pub userman_addr: Option<IpAddr>,
}

impl SshConfig {
    /// Load or generate the host key and build a server config.
    pub fn new(userman_addr_str: String) -> miette::Result<Self> {
        let host_key = hostkey::load_or_generate()?;

        let userman_addr = if userman_addr_str.is_empty() {
            None
        } else {
            Some(
                userman_addr_str
                    .parse::<IpAddr>()
                    .map_err(|e| miette!("Invalid usvc_ip: {e}"))?,
            )
        };

        Ok(Self {
            host_key,
            userman_addr,
        })
    }
}

/// The factory that creates a new [`SshHandler`] for each incoming connection.
struct SshServer {
    userman_addr: Option<IpAddr>,
}

impl russh::server::Server for SshServer {
    type Handler = SshHandler;

    fn new_client(&mut self, addr: Option<std::net::SocketAddr>) -> SshHandler {
        info!("New SSH connection from {addr:?}");
        let mut api = UserAPI::new();
        if let Some(addr) = self.userman_addr {
            api.set_addr(addr);
        }
        SshHandler::new(api)
    }
}

/// Start the SSH server on port 22.
pub async fn run(config: SshConfig) -> miette::Result<()> {
    let russh_config = russh::server::Config {
        keys: vec![config.host_key],
        auth_rejection_time: Duration::from_secs(3),
        auth_rejection_time_initial: Some(Duration::from_secs(0)),
        ..Default::default()
    };

    let mut server = SshServer {
        userman_addr: config.userman_addr,
    };

    info!("SSH server listening on 0.0.0.0:22");
    server
        .run_on_address(Arc::new(russh_config), ("0.0.0.0", 22))
        .await
        .into_diagnostic()?;

    Ok(())
}
