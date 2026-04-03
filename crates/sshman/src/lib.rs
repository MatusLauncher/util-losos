//! `sshman` — SSH server for losOS.
//!
//! Authenticates users against the userman HTTP daemon and enforces
//! Landlock session policies before spawning interactive shells.

pub mod auth;
pub mod handler;
pub mod hostkey;
pub mod mode;
pub mod server;
pub mod session;
