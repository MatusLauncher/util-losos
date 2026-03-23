use std::{
    fs,
    net::Ipv4Addr,
    path::Path,
    process::{self, Command},
    sync::Arc,
    thread,
    time::Duration,
};

use actman::cmdline::CmdLineOptions;
use miette::{IntoDiagnostic, miette};
use rustyx::RustyX;
use serde_json::json;
use tracing::{error, info, warn};

use crate::{
    PORT,
    schemas::{ClientState, CluManSchema, Mode, Task},
};

// ── Executor trait ────────────────────────────────────────────────────────────

/// Runs a Docker Compose file given its path on disk.
///
/// The single method receives a path to a temporary compose file that the
/// client has already written from the task payload.  It must start the stack
/// (real: `docker compose -f <path> up -d --remove-orphans`) and return
/// `Ok(())` on success or a human-readable error string on failure.
///
/// Annotated with `#[cfg_attr(test, mockall::automock)]` so that integration
/// tests can inject a [`MockExecutor`] and assert invocation details without
/// requiring Docker to be present on the test runner.
#[cfg_attr(test, mockall::automock)]
pub trait Executor: Send + Sync {
    /// Execute the compose stack described by `compose_file`.
    fn run_compose(&self, compose_file: &Path) -> Result<(), String>;
}

// ── ProcessExecutor ───────────────────────────────────────────────────────────

/// Production [`Executor`] that shells out to `docker compose`.
pub struct ProcessExecutor;

impl Executor for ProcessExecutor {
    /// Invoke `docker compose -f <compose_file> up -d --remove-orphans`.
    ///
    /// Returns `Ok(())` when the child process exits with status 0, or an
    /// `Err` containing the exit code and captured stderr otherwise.
    fn run_compose(&self, compose_file: &Path) -> Result<(), String> {
        let path_str = compose_file.to_string_lossy().into_owned();
        match Command::new("/bin/nerdctl")
            .args(["compose", "-f", &path_str, "up", "-d", "--remove-orphans"])
            .output()
        {
            Ok(out) if out.status.success() => Ok(()),
            Ok(out) => Err(format!(
                "exit {}: {}",
                out.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&out.stderr),
            )),
            Err(e) => Err(format!("failed to spawn `docker`: {e}")),
        }
    }
}

#[allow(unused)]
// ── Client ────────────────────────────────────────────────────────────────────
//
// Long-running.  Registers with a server on start-up, then polls it for tasks.
// Each received Task has its compose file content written to a temp file,
// executed via the injected Executor, and cleaned up afterwards.
//
// /proc/cmdline keys:
//   server_url=http://HOST:PORT   (required)
//   own_ip=<ipv4>                 (used in registration payload)
//
// Exposes:
//   GET /status   — health check

/// Entry point called from `main.rs`.
///
/// Delegates to [`run_client_with`] using the production [`ProcessExecutor`].
pub(crate) async fn run_client(cmdline: &CmdLineOptions) -> miette::Result<()> {
    run_client_with(cmdline, Arc::new(ProcessExecutor)).await
}

/// Core client logic parameterised over an [`Executor`].
///
/// Accepting `Arc<dyn Executor>` instead of hard-coding [`ProcessExecutor`]
/// lets tests pass a [`MockExecutor`] to verify task execution without
/// requiring Docker on the test host.
pub async fn run_client_with(
    cmdline: &CmdLineOptions,
    executor: Arc<dyn Executor>,
) -> miette::Result<()> {
    let server_url = cmdline.opts().get("server_url").cloned().ok_or_else(|| {
        miette!("No server URL in /proc/cmdline — add server_url=http://HOST:PORT")
    })?;

    let own_ip: Ipv4Addr = cmdline
        .opts()
        .get("own_ip")
        .and_then(|s| s.parse().ok())
        .unwrap_or(Ipv4Addr::LOCALHOST);

    info!(%own_ip, server_url, "Client starting");

    let state = ClientState::new(server_url.clone(), own_ip);
    let server = RustyX::new();

    // ── Register with the server ──────────────────────────────────────────────

    let payload = serde_json::to_string(&CluManSchema::registration(own_ip, Mode::Client))
        .into_diagnostic()?;

    match minreq::post(format!("{server_url}/api/register-client"))
        .with_header("Content-Type", "application/json")
        .with_body(payload)
        .send()
    {
        Ok(resp) => info!(status = resp.status_code, "Registered with server"),
        Err(e) => warn!(error = %e, "Failed to register with server — will retry next poll"),
    }

    // ── Background: poll server for tasks every 10 s ──────────────────────────
    //
    // minreq is synchronous; keep this in a plain OS thread so it does not
    // block the Tokio executor.  The Arc makes the executor shareable across
    // the thread boundary without cloning the underlying object.

    let st = state.clone();
    thread::spawn(move || {
        loop {
            match minreq::get(format!("{}/task", st.server_url)).send() {
                Ok(resp) if resp.status_code == 200 => {
                    if let Ok(body) = resp.as_str() {
                        match serde_json::from_str::<Task>(body) {
                            Ok(task) => {
                                info!(filename = task.filename, "Received task from server");
                                execute_compose(task, executor.as_ref());
                            }
                            Err(e) => warn!(error = %e, "Failed to parse task from server"),
                        }
                    }
                }
                Ok(resp) if resp.status_code == 204 => {} // no tasks yet — normal
                Ok(resp) => warn!(status = resp.status_code, "Unexpected status polling task"),
                Err(e) => warn!(error = %e, "Failed to poll server for task"),
            }
            thread::sleep(Duration::from_secs(10));
        }
    });

    // ── GET /status — health check ────────────────────────────────────────────

    server.get("/status", move |_req, res| async move {
        res.json(json!({ "status": "ok", "mode": "client" }))
    });

    info!(port = PORT, "Client listening");
    server.listen(PORT).await.into_diagnostic()
}

// ── Task execution ────────────────────────────────────────────────────────────

/// Write the task's compose file content to a temp file, delegate execution to
/// `executor`, then delete the temp file.
///
/// Runs synchronously inside the polling thread; at most one compose project is
/// started at a time per client node.  The temp file name embeds the process ID
/// and the original filename to avoid collisions if concurrent execution is ever
/// introduced.
fn execute_compose(task: Task, executor: &(impl Executor + ?Sized)) {
    let tmp_path = std::env::temp_dir().join(format!("cluman-{}-{}", process::id(), task.filename));

    if let Err(e) = fs::write(&tmp_path, &task.content) {
        error!(
            filename = task.filename,
            path     = %tmp_path.display(),
            error    = %e,
            "Failed to write compose file to temp path"
        );
        return;
    }

    match executor.run_compose(&tmp_path) {
        Ok(()) => {
            info!(filename = task.filename, "docker compose up succeeded");
        }
        Err(msg) => {
            error!(
                filename = task.filename,
                error    = %msg,
                "docker compose up failed"
            );
        }
    }

    if let Err(e) = fs::remove_file(&tmp_path) {
        warn!(
            path  = %tmp_path.display(),
            error = %e,
            "Failed to remove temp compose file"
        );
    }
}
