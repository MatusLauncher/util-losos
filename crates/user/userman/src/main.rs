use std::{
    env::args,
    io::{BufRead, Write, stdin, stdout},
    net::IpAddr,
    os::unix::process::CommandExt,
    path::Path,
    process::Command,
    str::FromStr,
};

use actman::cmdline::CmdLineOptions;
use clap::Parser;
use miette::{IntoDiagnostic, miette};
use perman::apply_session_policy;
use tracing_subscriber::fmt;
use userman::{
    cli::{ArgsParse, Mode},
    daemon::{Daemon, TwoFA, UserAPI, UserSchema, What},
    login::run_login_screen,
    mode::ModeOfOperation,
    twofa::{generate_totp_secret, register_fido2, totp_setup_uri},
};

/// Entry point.
///
/// Reads the process name from `argv[0]`, converts it to a
/// [`ModeOfOperation`], and dispatches accordingly.  The optional `usvc_ip`
/// kernel command-line option overrides the daemon address used by the client
/// and the login screen.
#[tokio::main]
async fn main() -> miette::Result<()> {
    let pname = Path::new(&args().collect::<Vec<_>>()[0])
        .file_name()
        .unwrap()
        .display()
        .to_string();
    fmt().init();

    let mut daemon_api = UserAPI::new();
    let cmdline = CmdLineOptions::new()?;
    let userdb_addr = cmdline.opts().get("usvc_ip").cloned().unwrap_or_default();

    match ModeOfOperation::from(pname) {
        ModeOfOperation::Client => {
            if !userdb_addr.is_empty() {
                daemon_api.set_addr(IpAddr::from_str(&userdb_addr).into_diagnostic()?);
            }
            let cli = ArgsParse::parse();
            match cli.mode() {
                Mode::Create {
                    name,
                    pass,
                    persistent_directories,
                    encrypt,
                    twofa,
                    second_pass,
                    luks_device,
                } => {
                    let mut schema = UserSchema::default();
                    schema
                        .set_user(name.clone())
                        .set_pass(pass.clone())
                        .set_allowed_dirs(persistent_directories.clone())
                        .set_encryption(*encrypt);
                    daemon_api.create_user(&schema)?;

                    if let Some(method) = twofa {
                        apply_twofa(&daemon_api, name, method, second_pass.as_deref())?;
                    }
                    if let Some(device) = luks_device {
                        daemon_api.update_user(name, What::LuksDevice(device.clone()))?;
                    }
                }
                Mode::Delete { name } => {
                    daemon_api.delete_user(name)?;
                }
                Mode::Update {
                    name,
                    new_pass,
                    locked_out,
                    allowed_dirs,
                    twofa,
                    disable_twofa,
                    second_pass,
                    luks_device,
                } => {
                    if let Some(pass) = new_pass {
                        daemon_api.update_user(name, What::Password(pass.clone()))?;
                    }
                    if let Some(status) = locked_out {
                        daemon_api.update_user(name, What::LockoutStatus(*status))?;
                    }
                    if let Some(dirs) = allowed_dirs {
                        daemon_api.update_user(name, What::AllowedDirectories(dirs.clone()))?;
                    }
                    if *disable_twofa {
                        daemon_api.update_user(name, What::TwoFactor(None))?;
                    } else if let Some(method) = twofa {
                        apply_twofa(&daemon_api, name, method, second_pass.as_deref())?;
                    }
                    if let Some(device) = luks_device {
                        daemon_api.update_user(name, What::LuksDevice(device.clone()))?;
                    }
                }
            }
        }
        ModeOfOperation::Daemon(loc) => {
            let daemon = Daemon::new(loc);
            daemon.run().await.unwrap();
        }
        ModeOfOperation::LoginScreen => {
            if !userdb_addr.is_empty() {
                daemon_api.set_addr(IpAddr::from_str(&userdb_addr).into_diagnostic()?);
            }
            let user = run_login_screen(&daemon_api)?;
            apply_session_policy(user.allowed_dirs())?;
            exec_shell()?;
        }
    }
    Ok(())
}

/// Print `label` (without newline), flush stdout, then read one line from stdin.
/// Returns the trimmed line.
fn prompt(label: &str) -> miette::Result<String> {
    print!("{label}");
    stdout().flush().into_diagnostic()?;
    let mut buf = String::new();
    stdin().lock().read_line(&mut buf).into_diagnostic()?;
    Ok(buf.trim_end_matches(['\n', '\r']).to_string())
}

/// Replace the current process image with `/bin/sh`, inheriting the
/// environment.  Only returns if `execve` fails (login is over either way).
fn exec_shell() -> miette::Result<()> {
    let err = Command::new("/bin/sh").exec();
    Err(miette!("execve(/bin/sh) failed: {err}"))
}

/// Handle `--twofa` for both `create` and `update` subcommands.
fn apply_twofa(
    api: &UserAPI,
    name: &str,
    method: &str,
    second_pass: Option<&str>,
) -> miette::Result<()> {
    match method {
        "totp" => {
            let secret = generate_totp_secret();
            let uri = totp_setup_uri(&secret, name)?;
            println!("TOTP setup URI:\n  {uri}");
            println!("Scan this with your authenticator app (or paste the URI manually).");
            api.update_user(name, What::TOTPSecret(secret))?;
            api.update_user(name, What::TwoFactor(Some(TwoFA::TOTP)))?;
        }
        "password" => {
            let sp = second_pass
                .ok_or_else(|| miette!("--second-pass is required with --twofa password"))?;
            api.update_user(name, What::SecondPassword(sp.to_string()))?;
            api.update_user(name, What::TwoFactor(Some(TwoFA::Password)))?;
        }
        "passkey" => {
            let pin_input = prompt("FIDO2 PIN (leave empty if not required): ")?;
            let pin_opt: Option<&str> = if pin_input.is_empty() {
                None
            } else {
                Some(&pin_input)
            };
            println!("Please touch your FIDO2 key when it blinks...");
            let (cred_id, pubkey_der, pubkey_type) = register_fido2(pin_opt)?;
            api.update_user(
                name,
                What::FIDOCredential {
                    credential_id: cred_id,
                    public_key_der: pubkey_der,
                    public_key_type: pubkey_type,
                },
            )?;
            api.update_user(name, What::TwoFactor(Some(TwoFA::Passkey)))?;
        }
        other => {
            return Err(miette!(
                "Unknown 2FA method '{other}'. Use: totp, password, passkey"
            ));
        }
    }
    Ok(())
}
