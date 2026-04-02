//! Command-line argument definitions for the `userman` CLI client.
//!
//! Parsed by [`clap`] using the derive API. The top-level struct is
//! [`ArgsParse`]; the subcommand is exposed via [`ArgsParse::mode`].

use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// Top-level argument parser for the `userman` CLI.
#[derive(Parser)]
#[clap(version, about, long_about = None)]
pub struct ArgsParse {
    #[command(subcommand)]
    mode: Mode,
}

impl ArgsParse {
    /// Return the chosen subcommand.
    pub fn mode(&self) -> &Mode {
        &self.mode
    }
}

/// Available subcommands.
#[derive(Subcommand)]
pub enum Mode {
    Create {
        /// User name.
        #[arg(short, long)]
        name: String,
        /// User password.
        #[arg(long)]
        pass: String,
        /// Directories that user has access to.
        #[arg(short)]
        persistent_directories: Vec<PathBuf>,
        /// Encrypt the home directory.
        #[arg(short, long, default_value_t = true)]
        encrypt: bool,
        /// Two-factor authentication method: totp, password, passkey.
        #[arg(long)]
        twofa: Option<String>,
        /// Second password (required when --twofa password).
        #[arg(long)]
        second_pass: Option<String>,
        /// LUKS block device path for the encrypted home directory.
        #[arg(long)]
        luks_device: Option<PathBuf>,
    },
    Delete {
        /// User name
        #[arg(short, long)]
        name: String,
    },
    Update {
        /// User name
        #[arg(long)]
        name: String,
        /// New password
        #[arg(long)]
        new_pass: Option<String>,
        /// Update lockout status
        #[arg(long)]
        locked_out: Option<bool>,
        /// Update allowed directories.
        #[arg(long)]
        allowed_dirs: Option<Vec<PathBuf>>,
        /// Set or change 2FA method: totp, password, passkey.
        #[arg(long)]
        twofa: Option<String>,
        /// Disable 2FA entirely.
        #[arg(long)]
        disable_twofa: bool,
        /// New second password (required when --twofa password).
        #[arg(long)]
        second_pass: Option<String>,
        /// LUKS block device path for the encrypted home directory.
        #[arg(long)]
        luks_device: Option<PathBuf>,
    },
}
