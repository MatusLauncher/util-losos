# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```bash
# Build
cargo build -p userman
cargo build -p perman

# Lint (project standard)
cargo clippy -p userman -- -W clippy::all -W clippy::perf

# Check without producing artifacts
cargo check -p userman
cargo check -p perman
```

There are no tests at present.

## Workspace layout

```
crates/
├── userman/    # Main user management binary (CLI + daemon + login screen)
└── perman/     # Permission enforcement library (cdylib, C FFI)
```

## Architecture

The workspace has two crates:

### userman

One binary serving four roles determined at runtime by the executable's filename:

| Symlink name | Role |
|---|---|
| `userman` / `useradd` | CLI client |
| `usersvc-local` | Daemon (local-only connections) |
| `usersvc-remote` | Daemon (remote connections) |
| `login` | Login / PAM-style screen |

**`mode.rs`** — `ModeOfOperation::from(exe_name)` drives dispatch in `main`. `Location` enum (`Local`/`Remote`) is carried by the `Daemon` variant.

**`daemon.rs`** — Both sides of the client/server split:
- `Daemon` — RustyX async HTTP server that persists `Users { uschemas: Vec<UserSchema> }` as JSON. Listens on port 20.
  - `GET  /healthcheck`
  - `GET  /user/get/:name` → `UserSchema`
  - `GET  /users` → `Vec<UserSchema>`
  - `POST /user/create` — body: `UserSchema`
  - `DELETE /user/delete/:name`
  - `PATCH /user/update/:name` — body: `ChangeSchema { what: What }`
- `UserAPI` — thin `ureq` HTTP client used by the CLI.
- `What` enum (update wire type): `Password(String)`, `LockoutStatus(bool)`, `AllowedDirectories(Vec<PathBuf>)`, `TwoFactor(Option<TwoFA>)`, `TOTPSecret(Vec<u8>)`, `FIDOCredential { credential_id, public_key_der, public_key_type }`, `SecondPassword(String)`, `LuksDevice(PathBuf)`.
- `validate_location` enforces loopback-only for local daemon, non-loopback-only for remote daemon.

**`cli.rs`** — Clap CLI with three subcommands: `Create`, `Delete`, `Update`. Notable flags:
- `Create`: `--name`, `--pass`, `-p persistent_directories`, `--encrypt` (default true), `--twofa [totp|password|passkey]`, `--second-pass`, `--luks-device`
- `Update`: `--name`, `--new-pass`, `--locked-out`, `--allowed-dirs`, `--twofa`, `--disable-twofa`, `--second-pass`, `--luks-device`

**`main.rs`** — Entry point. Reads `CmdLineOptions` for `usvc_ip`. Dispatches on mode:
- **Client** — parses CLI args, calls `UserAPI`, runs `apply_twofa` helper for 2FA setup.
- **Daemon** — constructs `Daemon` with location, calls `daemon.run()`.
- **LoginScreen** — interactive loop: prompts username + password, validates lockout, validates 2FA (TOTP / Password / Passkey), unlocks LUKS home if `encryption == true`.

**`twofa.rs`** — 2FA implementations:
- `generate_totp_secret() -> Vec<u8>` — 20 random bytes.
- `totp_setup_uri(secret, username) -> String` — `otpauth://` URI for QR setup.
- `validate_totp(secret, code) -> bool` — SHA1, 6 digits, 30-second window.
- `register_fido2(pin) -> (credential_id, public_key_der, public_key_type)` — CTAP HID registration; RPID = `"losOS"`.
- `verify_fido2(credential_id, public_key_der, public_key_type, pin) -> bool` — CTAP HID assertion.

**`crypto.rs`** — `unlock_home(device, passphrase, map_name)` — opens a LUKS2 container and maps it via dmsetup.

### perman

Compiles as a `cdylib` (C-compatible shared library). Intercepts `chdir` calls via a `#[no_mangle] extern "C" fn chdir` to enforce per-user `allowed_dirs`. Uses `USVC_IP` (lazy-loaded from `CmdLineOptions`) to call the `userman` daemon and validate the target path.

## Data model

```
UserSchema {
    user: String,
    pass: String,                    // base64-encoded (not hashed)
    allowed_dirs: Vec<PathBuf>,
    locked_out: bool,
    encryption: bool,
    twofa: Option<TwoFA>,            // Passkey | Password | TOTP
    totp_secret: Option<Vec<u8>>,
    fido2_credential_id: Option<Vec<u8>>,
    fido2_public_key_der: Option<Vec<u8>>,
    fido2_public_key_type: Option<u8>,  // 0=Unknown 1=Ecdsa256 2=Ed25519
    second_pass: Option<String>,
    luks_device: Option<PathBuf>,
}
```

## Key dependencies

- **RustyX** — custom async HTTP framework (internal, `rustyx = "*"`)
- **actman** — from `util-mdl` GitLab monorepo; actor/service management utilities
- **miette** — rich error diagnostics (`fancy` feature)
- **ureq** — sync HTTP client used in `UserAPI`
- **luks** — LUKS2 encryption via `unlock_home`
- **totp-rs** — TOTP generation/validation (`otpauth` feature)
- **ctap-hid-fido2** — FIDO2 hardware key registration and verification
- **tracing / tracing-subscriber** — structured logging
