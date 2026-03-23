# LosOS Utilities

`util-mdl` is the main repository where all LosOS OS utilities are built. It produces a bootable initramfs image containing an init system, a DHCP client, a cluster manager, and an OTA update manager — all compiled as statically-linked MUSL binaries and assembled into a cpio initramfs via a multi-stage container build.

## Crates

| Crate | Description |
|-------|-------------|
| [`actman`](#actman) | PID 1 init system — mounts filesystems, spawns startup scripts, handles shutdown |
| [`dhcman`](#dhcman) | DHCP client — configures a network interface via a full DORA sequence |
| [`cluman`](#cluman) | Cluster manager — client, server, and controller modes |
| [`updman`](#updman) | OTA update manager — pulls a new initramfs from a container registry and swaps it onto the BOOT partition |
| [`isoman`](#isoman) | ISO builder — generates the Containerfile, builds the initramfs via `podman build`, and assembles a hybrid BIOS+UEFI ISO with Limine |
| [`testman`](#testman) | Integration test framework — boots the initramfs in QEMU and asserts expected log output |
| [`bench`](#bench) | Criterion benchmarks for all crates |

## Building

```bash
# Standard debug build
cargo build

# Release build
cargo build --release

# Static MUSL target (used inside the initramfs)
cargo build --release --target x86_64-unknown-linux-musl

# Build initramfs + ISO in one step (requires podman)
cargo r -p isoman -- --build -m <client|server|controller>

# Build only the initramfs container image directly
podman build --no-cache --build-arg "MODE=client" -t util-mdl-build .
```

### launch.sh

`launch.sh` is a convenience wrapper for local development and CI.

```bash
# Launch the initramfs interactively in QEMU
./launch.sh

# Build initramfs first, then launch
./launch.sh --build

# Run integration tests (testman)
./launch.sh --test

# Build initramfs then run tests
./launch.sh --build --test

# Build a bootable ISO
./launch.sh --iso

# Disable KVM (CI environments without nested virtualisation)
KVM=0 ./launch.sh --test
```

Environment variables respected by `launch.sh`:

| Variable | Default | Description |
|----------|---------|-------------|
| `KERNEL` | `/boot/vmlinuz-$(uname -r)` | Path to the kernel image |
| `MEMORY` | `2G` | QEMU memory |
| `CPUS` | `2` | QEMU vCPU count |
| `KVM` | `1` | Set to `0` to disable `-enable-kvm` |
| `OUTPUT` | `os.iso` | ISO output path (used with `--iso`) |

## Lint and Format

```bash
cargo clippy
cargo fmt --check
```

## CI Pipeline

The GitLab CI pipeline is split into five focused stages:

| Job | Stage | Description |
|-----|-------|-------------|
| `compile` | `build` | Compiles the `isoman` binary on the Podman image |
| `initramfs` | `build` | Runs `podman build` to produce `os.initramfs.tar.gz` |
| `iso` | `assemble` | Assembles the hybrid ISO using `isoman` + Limine (skipped when `$KERNEL` is unset) |
| `test-boot` | `test` | Boots the initramfs in QEMU via `testman` (manual, requires `$KERNEL`) |
| `publish` | `publish` | Pushes the container image to the GitLab registry tagged as `<branch-slug>` |
| `publish-latest` | `publish` | Re-tags the branch image as `:latest` (default branch only) |

`compile` and `initramfs` run in parallel (same `build` stage). `iso` and `test-boot` each `needs:` only the artifact they actually consume, so the pipeline parallelises where possible.

Set `MODE` (CI/CD variable) to `client`, `server`, or `controller` to control which `cluman` mode is baked into the initramfs. Defaults to `client`.

---

## Architecture

### actman

`actman` is PID 1. A single binary dispatches on its `argv[0]` basename (symlink polymorphism):

| Basename | Role |
|----------|------|
| `init` | Mounts pseudo-filesystems, then walks `/etc/init/start/` and spawns each script. Loops forever reaping zombie children. |
| `poweroff` | Walks `/etc/init/stop/`, then calls `reboot(RB_POWER_OFF)`. |
| `reboot` | Walks `/etc/init/stop/`, then calls `reboot(RB_AUTOBOOT)`. |

Symlinks in the image: `/bin/poweroff → /bin/init`, `/bin/reboot → /bin/init`, `/init → bin/init`.

Key source files:

- `crates/actman/src/main.rs` — entry point and mode dispatch
- `crates/actman/src/preboot.rs` — filesystem mounting
- `crates/actman/src/reboot.rs` — `RebootCMD` enum → `rustix` syscall mapping
- `crates/actman/src/cmdline.rs` — `/proc/cmdline` parser

### dhcman

`dhcman` configures a network interface via DHCP. Like `actman`, it uses symlink polymorphism — `argv[0]` is treated as the interface name to configure.

```
ln -sf /bin/dhcman /etc/init/start/00-eth0   # configures eth0 at boot
```

At startup it waits up to 10 s for the interface to appear in sysfs (allowing for driver probe delays), then performs a full DORA exchange and configures the address, default route, and `/etc/resolv.conf`.

Key source files:

- `crates/dhcman/src/dhcp.rs` — DORA sequence using `dhcproto`
- `crates/dhcman/src/netconf.rs` — address/route configuration via netlink
- `crates/dhcman/src/main.rs` — entry point

### cluman

`cluman` is the cluster manager. It also uses symlink polymorphism — `argv[0]` selects the operating mode:

| Basename | Mode | Description |
|----------|------|-------------|
| `client` | Boot-time daemon | Registers with the server, polls for tasks, and executes Docker Compose files |
| `server` | Boot-time daemon | Maintains a client registry and a FIFO task queue; exposes an HTTP API on port 9999 |
| `controller` | One-shot CLI | Sends commands to the server (register, push tasks, query state) |

Client and server read configuration from `/proc/cmdline` at startup. The controller is configured entirely via `clap` CLI arguments.

The `Executor` trait abstracts `docker compose` execution, making the client fully testable without Docker via injected spy or mock executors (`mockall`).

Key source files:

- `crates/cluman/src/schemas.rs` — `Mode`, `Task`, `Tasks`, `ServerState`, `ClientState`, `IpRange`
- `crates/cluman/src/server.rs` — Tokio HTTP server
- `crates/cluman/src/client.rs` — client polling loop + `Executor` trait
- `crates/cluman/src/controller.rs` — one-shot controller

### updman

`updman` performs an over-the-air update of the initramfs on the `BOOT` partition. It reads `/etc/update.json`, runs `nerdctl save` to pull and export the container image, extracts the nested `os.initramfs.tar.gz` layer, mounts `/dev/disk/by-label/BOOT`, and replaces the initramfs file. The update takes effect on the next boot.

Key source files:

- `crates/updman/src/schema.rs` — `UpdMan` struct and `update()` orchestration
- `crates/updman/src/main.rs` — entry point

### isoman

`isoman` has two responsibilities:

1. **Build the initramfs** — generate the Containerfile from the template embedded in `crates/isoman/src/schema.rs`, bake in the chosen `cluman` mode (`ARG MODE=<mode>`), and invoke `podman build`.
2. **Assemble the ISO** — clone the Limine bootloader, copy the kernel and initramfs into a staging directory, and call `xorriso` to produce a hybrid BIOS+UEFI ISO.

The Containerfile is never read from disk; it lives as a `static` string in the Rust source and is written to a temp file before the build.

Key source files:

- `crates/isoman/src/schema.rs` — `CONT_F` template + `ContMode` renderer
- `crates/isoman/src/container.rs` — `podman build` invocation
- `crates/isoman/src/build.rs` — Limine ISO assembly
- `crates/isoman/src/main.rs` — `clap` CLI entry point

### testman

`testman` boots the initramfs in QEMU (with `-nographic` + `console=ttyS0`) and asserts that expected log lines appear within configurable timeouts. It is the primary end-to-end verification for `actman`.

```bash
# Run tests against a pre-built initramfs
./launch.sh --test

# Build then test
./launch.sh --build --test

# CI without KVM
KVM=0 ./launch.sh --test
```

Tests run sequentially against a single QEMU instance (mirroring real boot order). `testman` exits `0` on full pass, `1` on any failure or timeout — suitable for CI.

Key source files:

- `crates/testman/src/harness.rs` — `TestHarness` wraps the QEMU child process, drains stdout via a background thread, and exposes `wait_for(pattern, timeout)`

### bench

Criterion benchmarks for the core logic of `actman`, `dhcman`, `cluman`, and `updman`. Run with:

```bash
cargo bench
```

Results are written to `target/criterion/` as HTML reports.

---

## Error Handling and Logging

All fallible operations return `miette::Result<()>` using the `IntoDiagnostic` trait. Structured logging uses the `tracing` crate (`info!`, `warn!`, `error!`) initialised via `tracing-subscriber`.

## Container Build

The multi-stage Containerfile (embedded in `isoman`) produces `os.initramfs.tar.gz`:

| Stage | Base | Purpose |
|-------|------|---------|
| `stage0` | `alpine:latest` | Downloads `busybox-static` and the latest `nerdctl` full bundle; builds the target filesystem tree |
| `util` | `rust:alpine` | Compiles `actman`, `updman`, `dhcman`, `cluman` for `x86_64-unknown-linux-musl` |
| `stage1` | `alpine:latest` | Assembles the final filesystem, writes init scripts, creates symlinks, packs into a newc cpio archive |
| *(final)* | `scratch` | Exports `os.tar.gz` as `os.initramfs.tar.gz` |

The `MODE` build argument controls which `cluman` symlink is installed under `/etc/init/start/`, making the cluman role (client/server) a build-time decision baked into the image.

---

## AI Disclosure

This project uses generative AI (Claude Sonnet 4.6 Thinking) to generate documentation and tests. It is also used for refactoring and will be used in the future to review MRs.