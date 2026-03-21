# testman — Integration Test Framework

`testman` boots the initramfs in QEMU, reads serial output line by line, and asserts that expected log messages appear within configurable timeouts. It is the primary way to verify that `actman` (the init system) starts correctly end-to-end.

## Quick start

```bash
# Run against a pre-built initramfs
./launch.sh --test

# Build the initramfs first, then test
./launch.sh --build --test

# Disable KVM (CI environments without nested virtualisation)
KVM=0 ./launch.sh --test
```

Exit code is `0` when all tests pass, `1` when any test fails or times out — suitable for CI.

## How it works

```
launch.sh --test
    └─ cargo run -p testman
           ├─ TestSuite::run()         spawns one QEMU process
           │       └─ TestHarness      reads QEMU stdout in a background thread
           └─ test cases (sequential)  call harness.wait_for(pattern, timeout)
                                       each assertion consumes output up to the match
```

QEMU is launched in `-nographic` mode with `console=ttyS0`, so all kernel and init output appears on stdout. Tests run sequentially against the single QEMU instance, which mirrors the real boot order.

## Configuration

All options are passed as environment variables, matching `launch.sh` conventions.

| Variable    | Default                        | Description                              |
|-------------|--------------------------------|------------------------------------------|
| `KERNEL`    | `/boot/vmlinuz-$(uname -r)`   | Path to the kernel image (`vmlinuz`)     |
| `INITRAMFS` | `os.initramfs.tar.gz`         | Path to the initramfs tarball            |
| `MEMORY`    | `2G`                          | QEMU memory allocation                   |
| `CPUS`      | `2`                           | QEMU vCPU count                          |
| `KVM`       | `1`                           | Set to `0` to disable `-enable-kvm`      |

## Built-in test cases

Tests run in order. Each one calls `wait_for`, which scans QEMU output forward from where the previous test left off.

| Test name              | Pattern matched                    | Timeout |
|------------------------|------------------------------------|---------|
| `kernel boots`         | `Linux version`                    | 15 s    |
| `init starts`          | `Mounting`                         | 20 s    |
| `filesystems mounted`  | `Spawning`                         | 30 s    |
| `startup scripts run`  | `Spawning /etc/init/start/sh`      | 45 s    |

Patterns are substring matches against raw serial output lines. The `Mounting` and `Spawning` strings come from `tracing::info!` calls in `actman`.

## Output

```
--- Test Results ---
  PASS  kernel boots
  PASS  init starts
  PASS  filesystems mounted
  FAIL  startup scripts run — timeout
--- 3/4 passed ---
```

On failure the full captured QEMU output is available via `TestHarness::dump_log()` for diagnostics.

## Architecture

### `harness.rs` — `TestHarness`

Wraps a `Child` QEMU process. A background thread drains stdout into an `mpsc` channel. `wait_for` receives from the channel with a deadline, appending every line to an internal log.

```rust
pub fn wait_for(&mut self, pattern: &str, timeout: Duration) -> miette::Result<bool>
pub fn send(&mut self, line: &str) -> miette::Result<()>   // write to serial stdin
pub fn dump_log(&self) -> &[String]
pub fn shutdown(self)                                        // kills QEMU
```

`HarnessConfig` holds all QEMU parameters. It implements `Default` (`memory = "2G"`, `cpus = 2`, `kvm = true`).

### `suite.rs` — `TestSuite`

Builder that accumulates test closures and runs them sequentially against one harness.

```rust
TestSuite::new()
    .test("name", |h| { ... TestResult::Pass })
    .run(config)?
```

`TestResult` is `Pass`, `Fail(String)`, or `Timeout`. `TestReport::print()` writes the summary table; `has_failures()` drives the process exit code.

### `main.rs`

Reads env vars, constructs `HarnessConfig`, wires up the four built-in test cases, calls `suite.run()`, prints the report, and exits accordingly.

## Adding test cases

Add a `.test()` call in `crates/testman/src/main.rs`:

```rust
.test("nerdctl available", |h| {
    match h.wait_for("nerdctl", Duration::from_secs(60)) {
        Ok(true) => TestResult::Pass,
        Ok(false) => TestResult::Timeout,
        Err(e)   => TestResult::Fail(e.to_string()),
    }
})
```

To interact with the running system, use `h.send("command\n")` before waiting for the response.

## Building without QEMU

```bash
cargo build -p testman
```

The binary compiles on any host regardless of whether `qemu-system-x86_64` is installed. QEMU is only required at runtime.
