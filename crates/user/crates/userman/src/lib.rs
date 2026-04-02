//! `userman` — user account management for losOS.
//!
//! The compiled binary serves multiple roles depending on the name it is
//! invoked under (see [`mode::ModeOfOperation`]):
//!
//! | Symlink        | Role                                   |
//! |----------------|----------------------------------------|
//! | `userman`      | CLI client (create / delete / update)  |
//! | `useradd`      | alias for `userman`                    |
//! | `usersvc-local`| HTTP daemon, loopback connections only |
//! | `usersvc-remote`| HTTP daemon, remote connections only  |
//! | `login`        | Interactive login / PAM-style screen   |

pub mod cli;
pub mod crypto;
pub mod daemon;
pub mod login;
pub mod mode;
pub mod twofa;