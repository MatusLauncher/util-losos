//! Smoke tests for the `sshman` crate.
//!
//! Exercises:
//! * `ModeOfOperation::from` — executable-name-to-mode dispatch.
//! * `auth::verify_password` — base64-encoded password verification at
//!   various lengths.
//! * `auth::check_account` — locked/unlocked account checks.
//! * `auth::verify_second_password` — secondary password matching.
//! * `auth::verify_totp` — TOTP code validation.
//! * `auth::match_public_key` — SSH public-key matching with scaling.

use std::hint::black_box;

use base64::{Engine, prelude::BASE64_STANDARD};
use sshman::auth;
use sshman::mode::ModeOfOperation;
use userman::daemon::UserSchema;

// ── helpers ─────────────────────────────────────────────────────────

/// Build a `UserSchema` from JSON via serde, same pattern as sshman's
/// inline tests.
fn make_user(json: &str) -> UserSchema {
    serde_json::from_str(json).expect("valid user JSON")
}

fn unlocked_user(password: &str) -> UserSchema {
    let pass_b64 = BASE64_STANDARD.encode(password);
    make_user(&format!(
        r#"{{"user":"testuser","pass":"{pass_b64}","allowed_dirs":[],"locked_out":false,"encryption":false}}"#
    ))
}

fn locked_user() -> UserSchema {
    make_user(
        r#"{"user":"locked","pass":"","allowed_dirs":[],"locked_out":true,"encryption":false}"#,
    )
}

fn password_of_len(n: usize) -> String {
    "A".repeat(n)
}

// ── mode_dispatch ───────────────────────────────────────────────────

mod mode_dispatch {
    use super::*;

    #[test]
    fn sshman_bare() {
        assert_eq!(
            black_box(ModeOfOperation::from("sshman".to_string())),
            ModeOfOperation::Daemon
        );
    }

    #[test]
    fn sshd_bare() {
        assert_eq!(
            black_box(ModeOfOperation::from("sshd".to_string())),
            ModeOfOperation::Daemon
        );
    }

    #[test]
    fn unknown_bare() {
        assert_eq!(
            black_box(ModeOfOperation::from("openssh".to_string())),
            ModeOfOperation::Unknown
        );
    }

    #[test]
    fn empty() {
        assert_eq!(
            black_box(ModeOfOperation::from(String::new())),
            ModeOfOperation::Unknown
        );
    }

    #[test]
    fn full_path() {
        assert_eq!(
            black_box(ModeOfOperation::from("/usr/sbin/sshd".to_string())),
            ModeOfOperation::Unknown
        );
    }
}

// ── verify_password ─────────────────────────────────────────────────

mod verify_password {
    use super::*;

    #[test]
    fn correct() {
        let user = unlocked_user("hunter2");
        assert!(black_box(auth::verify_password(&user, "hunter2")));
    }

    #[test]
    fn wrong() {
        let user = unlocked_user("hunter2");
        assert!(!black_box(auth::verify_password(&user, "wrong")));
    }

    #[test]
    fn empty_matches_empty() {
        let user = unlocked_user("");
        assert!(black_box(auth::verify_password(&user, "")));
    }

    #[test]
    fn empty_rejects_nonempty() {
        let user = unlocked_user("");
        assert!(!black_box(auth::verify_password(&user, "something")));
    }

    #[test]
    fn short_password() {
        let pw = password_of_len(5);
        let user = unlocked_user(&pw);
        assert!(black_box(auth::verify_password(&user, &pw)));
    }

    #[test]
    fn medium_password() {
        let pw = password_of_len(64);
        let user = unlocked_user(&pw);
        assert!(black_box(auth::verify_password(&user, &pw)));
    }

    #[test]
    fn long_password() {
        let pw = password_of_len(256);
        let user = unlocked_user(&pw);
        assert!(black_box(auth::verify_password(&user, &pw)));
    }
}

// ── verify_password_scaling ─────────────────────────────────────────

mod verify_password_scaling {
    use super::*;

    #[test]
    fn scaling() {
        for n in [8usize, 32, 64, 128, 256] {
            let pw = password_of_len(n);
            let user = unlocked_user(&pw);
            assert!(black_box(auth::verify_password(&user, &pw)));
        }
    }
}

// ── check_account ───────────────────────────────────────────────────

mod check_account {
    use super::*;

    #[test]
    fn unlocked_ok() {
        let user = unlocked_user("pass");
        assert!(black_box(auth::check_account(&user)).is_ok());
    }

    #[test]
    fn locked_rejected() {
        let user = locked_user();
        assert!(black_box(auth::check_account(&user)).is_err());
    }
}

// ── verify_second_password ──────────────────────────────────────────

mod verify_second_password {
    use super::*;

    #[test]
    fn matches() {
        let mut user = unlocked_user("primary");
        user.set_second_pass("backup".to_string());
        assert!(black_box(auth::verify_second_password(&user, "backup")));
    }

    #[test]
    fn wrong() {
        let mut user = unlocked_user("primary");
        user.set_second_pass("backup".to_string());
        assert!(!black_box(auth::verify_second_password(&user, "nope")));
    }

    #[test]
    fn not_set() {
        let user = unlocked_user("primary");
        assert!(!black_box(auth::verify_second_password(
            &user, "anything"
        )));
    }
}

// ── verify_totp ─────────────────────────────────────────────────────

mod verify_totp {
    use super::*;

    /// Shared 20-byte TOTP secret.
    fn totp_secret() -> Vec<u8> {
        vec![
            0x48, 0x65, 0x6C, 0x6C, 0x6F, 0x21, 0xDE, 0xAD, 0xBE, 0xEF, 0x48, 0x65, 0x6C, 0x6C,
            0x6F, 0x21, 0xDE, 0xAD, 0xBE, 0xEF,
        ]
    }

    fn generate_valid_code(secret: &[u8]) -> String {
        use totp_rs::{Algorithm, TOTP};
        let totp = TOTP::new(
            Algorithm::SHA1,
            6,
            1,
            30,
            secret.to_vec(),
            Some("userman".to_string()),
            String::new(),
        )
        .unwrap();
        totp.generate_current().unwrap()
    }

    #[test]
    fn valid_code_accepted() {
        let secret = totp_secret();
        let mut user = unlocked_user("pass");
        user.set_totp_secret(secret.clone());
        let code = generate_valid_code(&secret);
        assert!(black_box(auth::verify_totp(&user, &code)).unwrap());
    }

    #[test]
    fn wrong_code_rejected() {
        let mut user = unlocked_user("pass");
        user.set_totp_secret(totp_secret());
        assert!(!black_box(auth::verify_totp(&user, "000000")).unwrap());
    }

    #[test]
    fn no_secret_errors() {
        let user = unlocked_user("pass");
        assert!(black_box(auth::verify_totp(&user, "123456")).is_err());
    }
}

// ── match_public_key ────────────────────────────────────────────────

mod match_public_key {
    use super::*;

    fn generate_ed25519_keypair() -> ssh_key::PrivateKey {
        ssh_key::PrivateKey::random(
            &mut ssh_key::rand_core::OsRng,
            ssh_key::Algorithm::Ed25519,
        )
        .unwrap()
    }

    fn openssh_line(key: &ssh_key::PrivateKey) -> String {
        key.public_key().to_openssh().unwrap().to_string()
    }

    #[test]
    fn match_found() {
        let key = generate_ed25519_keypair();
        let pub_key = key.public_key().clone();
        let mut user = unlocked_user("pass");
        user.set_ssh_public_keys(vec![openssh_line(&key)]);
        assert!(black_box(auth::match_public_key(&user, &pub_key)));
    }

    #[test]
    fn no_match() {
        let key1 = generate_ed25519_keypair();
        let key2 = generate_ed25519_keypair();
        let mut user = unlocked_user("pass");
        user.set_ssh_public_keys(vec![openssh_line(&key1)]);
        assert!(!black_box(auth::match_public_key(
            &user,
            key2.public_key()
        )));
    }

    #[test]
    fn empty_key_list() {
        let key = generate_ed25519_keypair();
        let user = unlocked_user("pass");
        assert!(!black_box(auth::match_public_key(
            &user,
            key.public_key()
        )));
    }

    #[test]
    fn malformed_key_skipped() {
        let key = generate_ed25519_keypair();
        let mut user = unlocked_user("pass");
        user.set_ssh_public_keys(vec!["not-a-valid-key".to_string()]);
        assert!(!black_box(auth::match_public_key(
            &user,
            key.public_key()
        )));
    }
}

// ── match_public_key_scaling ────────────────────────────────────────

mod match_public_key_scaling {
    use super::*;

    fn generate_ed25519_keypair() -> ssh_key::PrivateKey {
        ssh_key::PrivateKey::random(
            &mut ssh_key::rand_core::OsRng,
            ssh_key::Algorithm::Ed25519,
        )
        .unwrap()
    }

    fn openssh_line(key: &ssh_key::PrivateKey) -> String {
        key.public_key().to_openssh().unwrap().to_string()
    }

    #[test]
    fn scaling_match_at_end() {
        for n in [1usize, 5, 10, 25] {
            let target = generate_ed25519_keypair();
            let mut keys: Vec<String> = (0..n.saturating_sub(1))
                .map(|_| openssh_line(&generate_ed25519_keypair()))
                .collect();
            keys.push(openssh_line(&target));

            let mut user = unlocked_user("pass");
            user.set_ssh_public_keys(keys);
            assert!(black_box(auth::match_public_key(
                &user,
                target.public_key()
            )));
        }
    }

    #[test]
    fn scaling_no_match() {
        for n in [1usize, 5, 10, 25] {
            let needle = generate_ed25519_keypair();
            let keys: Vec<String> = (0..n)
                .map(|_| openssh_line(&generate_ed25519_keypair()))
                .collect();

            let mut user = unlocked_user("pass");
            user.set_ssh_public_keys(keys);
            assert!(!black_box(auth::match_public_key(
                &user,
                needle.public_key()
            )));
        }
    }
}
