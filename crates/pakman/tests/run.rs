//! Integration tests for `pakman::run::ProgRunner`.
//!
//! These tests exercise `ProgRunner` through its public API without a live
//! `nerdctl` installation or a real `/data/progs` package store.
//!
//! # What is tested here vs. in unit tests
//!
//! The `#[cfg(test)]` module inside `run.rs` verifies the internal
//! `find_tarball` helper in isolation using temporary directories.  These
//! integration tests complement that by driving `ProgRunner` through its
//! *public* interface only — the same surface that callers outside the crate
//! see.
//!
//! # Mocking strategy
//!
//! | Boundary | Approach |
//! |---|---|
//! | `nerdctl` subprocess | Not mocked — tests assert on the error or panic produced when the binary is absent. |
//! | `/data/progs` store | Not redirectable (path is hardcoded); tests rely on the well-defined panic documented in `ProgRunner::run`. |
//!
//! Tests that would require `nerdctl` to be installed are gated behind a
//! runtime guard and skipped when `/bin/nerdctl` is absent.

use std::mem;

use pakman::run::ProgRunner;

// ── helpers ───────────────────────────────────────────────────────────────────

/// Returns `true` when `/bin/nerdctl` exists on the current host.
///
/// Used to skip tests that exercise the live `nerdctl load` / `nerdctl run`
/// code paths, which are only available inside the initramfs QEMU environment.
fn nerdctl_available() -> bool {
    std::path::Path::new("/bin/nerdctl").exists()
}

/// Returns `true` when `/data/progs` exists on the current host.
///
/// The directory is only present inside a running initramfs with a mounted
/// data drive.
fn data_progs_exists() -> bool {
    std::path::Path::new("/data/progs").is_dir()
}

// ── construction ──────────────────────────────────────────────────────────────

/// `ProgRunner::new()` must not panic and must return a usable value.
#[test]
fn new_constructs_successfully() {
    let _runner = ProgRunner::new();
}

/// `ProgRunner::default()` must not panic and must return a usable value.
#[test]
fn default_constructs_successfully() {
    let _runner = ProgRunner::default();
}

/// `ProgRunner` carries no state of its own; its in-memory size must be zero.
#[test]
fn prog_runner_is_zero_sized() {
    assert_eq!(
        mem::size_of::<ProgRunner>(),
        0,
        "ProgRunner must be a zero-sized type — it stores no fields"
    );
}

/// A `ProgRunner` obtained via `new()` and one obtained via `default()` must
/// be interchangeable — both should behave identically since the type is a ZST.
#[test]
fn new_and_default_have_equal_size() {
    assert_eq!(
        mem::size_of_val(&ProgRunner::new()),
        mem::size_of_val(&ProgRunner::default()),
    );
}

// ── run — missing package (no /data/progs entry) ─────────────────────────────
//
// When the requested program has never been installed, `find_tarball` returns
// an empty Vec and the `.first().expect(...)` call panics.  This is documented
// behaviour; the tests below assert on it so any future change to that contract
// is immediately visible.

/// `run()` must panic when the program has never been installed.
///
/// This test does not require `nerdctl` — the panic happens before any
/// subprocess is spawned.
#[test]
fn run_panics_when_program_not_installed() {
    let runner = ProgRunner::new();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // Use a name that will never exist on any test host.
        let _ = runner.run("__pakman_test_nonexistent_program_xyz__");
    }));
    assert!(
        result.is_err(),
        "run() must panic when the program tarball is not present in /data/progs"
    );
}

/// The panic message must mention installation so the operator understands the
/// recovery action.
#[test]
fn run_panic_message_mentions_install() {
    let runner = ProgRunner::new();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = runner.run("__pakman_test_nonexistent_program_xyz__");
    }));

    let err = result.unwrap_err();

    // `catch_unwind` delivers the panic payload as `Box<dyn Any>`.
    // The payload is a `&str` when the panic originated from `expect()`.
    let msg = if let Some(s) = err.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = err.downcast_ref::<String>() {
        s.clone()
    } else {
        String::new()
    };

    assert!(
        msg.contains("install"),
        "panic message must mention 'install' to guide the operator, got: {msg:?}"
    );
}

/// `run()` must panic for every distinct uninstalled name — the guard is not
/// specific to a single package.
#[test]
fn run_panics_consistently_across_different_uninstalled_names() {
    let packages = ["aaaa_not_real", "bbbb_not_real", "cccc_not_real"];
    for pkg in &packages {
        let runner = ProgRunner::new();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = runner.run(pkg);
        }));
        assert!(
            result.is_err(),
            "run({pkg:?}) must panic when the tarball is absent"
        );
    }
}

/// Each `run()` call is independent — a panic from one invocation must not
/// poison a subsequently constructed `ProgRunner`.
#[test]
fn run_panic_does_not_affect_subsequent_runners() {
    // First runner — expected to panic.
    let result1 = std::panic::catch_unwind(|| {
        let runner = ProgRunner::new();
        let _ = runner.run("__first_nonexistent__");
    });
    assert!(result1.is_err());

    // Second runner — construction must succeed even after the first panicked.
    let result2 = std::panic::catch_unwind(|| {
        let runner = ProgRunner::new();
        let _ = runner.run("__second_nonexistent__");
    });
    assert!(
        result2.is_err(),
        "second runner must also panic for the missing package, not crash differently"
    );
}

// ── run — nerdctl error path ──────────────────────────────────────────────────
//
// The following tests require both `/data/progs` to exist and a matching
// tarball to be present.  They are skipped on the test host where neither
// condition holds.  On the initramfs QEMU system they will exercise the live
// `nerdctl load` call.

/// `run()` must return an error (not panic) when `nerdctl` is absent and a
/// tarball for the requested package does exist.
///
/// Skipped when `/data/progs` is not mounted or `nerdctl` is present (in which
/// case the test would need a real image and is out of scope for unit/CI runs).
#[test]
fn run_returns_err_when_nerdctl_absent_but_tarball_exists() {
    if !data_progs_exists() {
        // /data/progs is not mounted — skip.
        return;
    }
    if nerdctl_available() {
        // nerdctl is present; this test only covers the absent-nerdctl path.
        return;
    }

    // Find the first .tar file in /data/progs, if any, to manufacture a
    // program name that will match.
    let first_tar = walkdir::WalkDir::new("/data/progs")
        .into_iter()
        .filter_map(|e| e.ok())
        .find(|e| {
            e.path()
                .extension()
                .and_then(|x| x.to_str())
                .map(|x| x == "tar")
                .unwrap_or(false)
        });

    if let Some(entry) = first_tar {
        let stem = entry
            .path()
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("__unknown__")
            .to_owned();

        let runner = ProgRunner::new();
        let result = runner.run(&stem);
        assert!(
            result.is_err(),
            "run() must return Err when nerdctl is absent, got Ok"
        );
    }
    // If there are no tarballs yet, the test is vacuously satisfied.
}

// ── run — return type and error propagation ───────────────────────────────────

/// `run()` must return `miette::Result<()>` — confirm the Ok variant is `()`.
///
/// This test does not execute `run()` to completion (that would require
/// `nerdctl`), but it confirms the type signature at the call site.
#[test]
fn run_result_ok_type_is_unit() {
    // This is a compile-time shape test: if the signature ever changes from
    // `miette::Result<()>` to something else, this will fail to compile.
    fn _assert_return_type(_: &dyn Fn(&str) -> miette::Result<()>) {}

    let runner = ProgRunner::new();
    // We cannot call runner.run() safely here without triggering the panic, so
    // we use a closure that wraps it and confirm the type inference works.
    let _: fn(&ProgRunner, &str) -> miette::Result<()> = |r, p| r.run(p);
    drop(runner);
}

// ── run — concurrent construction ────────────────────────────────────────────

/// Multiple `ProgRunner` instances may be created concurrently without data
/// races.  Since `ProgRunner` is a ZST, construction is purely a type-system
/// operation with no heap activity.
#[test]
fn concurrent_construction_does_not_panic() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;

    let count = Arc::new(AtomicUsize::new(0));
    let threads: Vec<_> = (0..16)
        .map(|_| {
            let c = Arc::clone(&count);
            thread::spawn(move || {
                let _runner = ProgRunner::new();
                c.fetch_add(1, Ordering::Relaxed);
            })
        })
        .collect();

    for t in threads {
        t.join().expect("thread panicked");
    }

    assert_eq!(count.load(Ordering::Relaxed), 16);
}
