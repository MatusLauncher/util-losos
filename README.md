# LosOS Utilities

`util-mdl` is the main repository where all LosOS OS utilities are built. It produces a bootable initramfs image containing an init system, a DHCP client, a cluster manager, a user manager, and an OTA update manager — all compiled as statically-linked MUSL binaries and assembled into a cpio initramfs via a multi-stage container build.

## Crates

| Crate | Description |
|-------|-------------|
| [`actman`](#actman) | PID 1 init system — mounts filesystems, spawns startup scripts, handles shutdown |
| [`dhcman`](#dhcman) | DHCP client — configures a network interface via a full DORA sequence |
| [`cluman`](#cluman) | Cluster manager — client, server, and controller modes |
| [`updman`](#updman) | OTA update manager — pulls a new initramfs from a container registry and swaps it onto the BOOT partition |
| [`isoman`](#isoman) | ISO / GSI builder — generates the Containerfile, builds the initramfs via `podman build`, assembles a hybrid BIOS+UEFI ISO with Limine, and optionally produces Android-compatible GSI images via `mkbootimg` |
| [`userman`](#userman) | User manager — CLI client, HTTP daemon, and login screen with 2FA (TOTP / password / FIDO2) and LUKS home encryption |
| [`perman`](#perman) | Permission enforcement — `cdylib` that intercepts `chdir` via `LD_PRELOAD` to enforce per-user allowed directories |
| [`pakman`](#pakman) | Package manager — installs, removes, and runs programs packaged as Nix-based container images stored on the data drive |
| [`gpuman`](#gpuman) | GPU/NPU accelerator manager — detects GPUs and NPUs at boot via sysfs and launches vendor-specific driver containers (CUDA, ROCm, oneAPI) |
| [`testman`](#testman) | Integration test framework — boots the initramfs in QEMU and asserts expected log output |
| [`bench`](#bench) | Smoke tests and micro-benchmarks for all crates |

## Building

```bash
# Standard debug build
cargo build

# Release build
cargo build --release

# Static MUSL target (used inside the initramfs)
cargo build --release --target x86_64-unknown-linux-musl

# Build initramfs + ISO in one step (requires podman)
# Output files are mode-stamped: os-client.iso, os-server.iso, etc.
cargo run -p isoman -- --build --mode <client|server|controller>

# Build only the initramfs, skip ISO assembly
cargo run -p isoman -- --build --mode client --initramfs-out os-client.initramfs.tar.gz

# Assemble ISO from a pre-built initramfs
cargo run -p isoman -- --initramfs os-client.initramfs.tar.gz --mode client

# Build an Android GSI (boot.img + Odin .tar.md5) instead of an ISO
cargo run -p isoman -- --build --mode client --gsi
cargo run -p isoman -- --build --mode client --gsi --gsi-format fastboot
cargo run -p isoman -- --build --mode client --gsi --gsi-format odin
```

### Output filename conventions

All `isoman` output paths default to a mode-stamped name so that client and server artifacts built in the same working directory never overwrite each other:

| Artifact | Default path |
|----------|-------------|
| ISO | `os-<mode>.iso` (e.g. `os-client.iso`) |
| Initramfs | `os-<mode>.initramfs.tar.gz` (e.g. `os-server.initramfs.tar.gz`) |
| Fastboot GSI | `boot.img` |
| Odin GSI | `AP_losos.tar.md5` |

Pass `--output`, `--initramfs`, `--fastboot-out`, or `--odin-out` to override any of these.

### launch.sh

`launch.sh` is a convenience wrapper for local development and CI.

```bash
# Launch the initramfs interactively in QEMU
./launch.sh

# Build initramfs first (via podman build), then launch
./launch.sh --build

# Run integration tests (testman)
./launch.sh --test

# Build initramfs then run tests
./launch.sh --build --test

# Build a bootable ISO from a pre-built initramfs
./launch.sh --iso

# Disable KVM (CI environments without nested virtualisation)
KVM=0 ./launch.sh --test
```

`launch.sh` automatically builds a supplemental initrd containing decompressed `virtio_net`, `net_failover`, and `failover` kernel modules and prepends it to the initramfs so that the virtual NIC is available in QEMU even when the main image has no kernel modules.

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

The GitLab CI pipeline is split into focused stages:

| Job | Stage | Description |
|-----|-------|-------------|
| `compile` | `build` | Compiles the `isoman` binary on the Podman image |
| `initramfs` | `build` | Runs `podman build` to produce `os-<mode>.initramfs.tar.gz` |
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
| `client` | Boot-time daemon | Registers with the server, polls for tasks, executes Docker Compose with GPU + NIC injection |
| `server` | Boot-time daemon | Maintains a client registry and a **priority-aware** task queue; exposes an HTTP API on port 9999 |
| `controller` | One-shot CLI | Sends commands to the server (push tasks with priority, VLAN, and preemption flags) |

**New scaling features:**
- **Priority dispatch** — Tasks carry priority 0-255; higher priority tasks dispatched first
- **Task preemption** — High-priority tasks can preempt `preemptible` running tasks
- **NIC detection** — Automatic discovery of all network interfaces at boot (sysfs + netlink)
- **VLAN isolation** — Per-service VLAN tagging for network isolation between compose services
- **Multi-server HA** — Server peer configuration for basic leader/follower failover

Client and server read configuration from `/proc/cmdline` at startup. The controller is configured entirely via `clap` CLI arguments.

The `Executor` trait abstracts `docker compose` execution, making the client fully testable without Docker via injected spy or mock executors (`mockall`).

Key source files:

- `crates/cluman/src/schemas.rs` — `Mode`, `Task`, `Tasks`, `ServerState`, `ClientState`, `NetworkInterface`, `NicRequirements`, `IpRange`
- `crates/cluman/src/server.rs` — Tokio HTTP server with priority dispatch and preemption
- `crates/cluman/src/client.rs` — client polling loop + `Executor` trait + NIC detection + VLAN setup
- `crates/cluman/src/controller.rs` — one-shot controller with `--priority`, `--preemptible`, `--vlan-id` flags
- `crates/cluman/src/compose.rs` — GPU + NIC compose file rewriter
- `crates/cluman/src/detect.rs` — Netlink + sysfs NIC detection
- `crates/cluman/src/vlan.rs` — VLAN interface management

### updman

`updman` performs an over-the-air update of the initramfs on the `BOOT` partition. It reads `/etc/update.json`, runs `nerdctl save` to pull and export the container image, extracts the nested `os.initramfs.tar.gz` layer, mounts `/dev/disk/by-label/BOOT`, and replaces the initramfs file. The update takes effect on the next boot.

Key source files:

- `crates/updman/src/schema.rs` — `UpdMan` struct and `update()` orchestration
- `crates/updman/src/main.rs` — entry point

### isoman

`isoman` has three responsibilities:

1. **Build the initramfs** — generate the Containerfile from the template embedded in `crates/isoman/src/schema.rs`, bake in the chosen `cluman` mode (`ARG MODE=<mode>`), and invoke `podman build`. Output is mode-stamped (`os-<mode>.initramfs.tar.gz`) so client and server archives never collide.

2. **Assemble the ISO** — clone the Limine bootloader, copy the kernel and initramfs into a staging directory, and call `xorriso` to produce a hybrid BIOS+UEFI ISO. Output defaults to `os-<mode>.iso`.

3. **Build a GSI** — use the [`mkbootimg`](https://gitlab.com/losos-linux/mkbootimg) Rust library (wrapping the upstream Android `mkbootimg` C tool) to bundle the kernel and initramfs into an Android boot image. Supports two output formats:
   - **Fastboot** — raw `boot.img` flashable via `fastboot flash boot`.
   - **Odin** — `AP_losos.tar.md5` archive for Samsung Odin / `heimdall`.

The kernel auto-detection (`--kernel` omitted) walks `/boot` recursively and matches on the filename only, so it works correctly on standard distros (`/boot/vmlinuz-<release>`) as well as immutable ones where the kernel lives in a subdirectory (e.g. Silverblue's `/boot/ostree/<deployment>/vmlinuz-<release>`). The running release string from `uname -r` is used to prefer the booted kernel over stale deployments.

The Containerfile is never read from disk; it lives as `static` strings in the Rust source and is written to a temp file before the build.

Key source files:

- `crates/isoman/src/schema.rs` — `ContMode` renderer + embedded Containerfile stage constants
- `crates/isoman/src/container.rs` — `podman build` / `podman cp` invocation
- `crates/isoman/src/build.rs` — Limine ISO assembly
- `crates/isoman/src/gsi.rs` — GSI builder using the `mkbootimg` library
- `crates/isoman/src/main.rs` — `clap` CLI entry point; `find_kernel` auto-detection

### userman

`userman` is the user management subsystem. A single binary serves four roles determined at runtime by the executable's filename (symlink polymorphism):

| Symlink name | Role |
|---|---|
| `userman` / `useradd` | CLI client — `create`, `delete`, `update` subcommands |
| `usersvc-local` | HTTP daemon — loopback-only connections |
| `usersvc-remote` | HTTP daemon — non-loopback connections |
| `login` | Interactive login screen with 2FA and LUKS home unlock |

The daemon exposes a REST API on port 20 and persists user records as JSON:

| Method | Endpoint | Description |
|--------|----------|-------------|
| `GET` | `/healthcheck` | Liveness probe |
| `GET` | `/user/get/:name` | Fetch a single user record |
| `GET` | `/users` | List all users |
| `POST` | `/user/create` | Create a user |
| `DELETE` | `/user/delete/:name` | Remove a user |
| `PATCH` | `/user/update/:name` | Update a user field |

Supported 2FA methods: **TOTP** (SHA1, 6-digit, 30-second window), **secondary password**, and **FIDO2/Passkey** (CTAP HID, RPID `"losOS"`). When `--encrypt` is set at creation time, the login screen unlocks a LUKS2 home partition before completing the session.

Key source files:

- `crates/user/userman/src/mode.rs` — `ModeOfOperation::from(exe_name)` dispatch
- `crates/user/userman/src/daemon.rs` — `Daemon` (expressjs HTTP server) and `UserAPI` (ureq client)
- `crates/user/userman/src/cli.rs` — clap CLI (`Create`, `Delete`, `Update`)
- `crates/user/userman/src/twofa.rs` — TOTP generation/validation, FIDO2 registration/assertion
- `crates/user/userman/src/crypto.rs` — LUKS2 home unlock via dmsetup

### perman

`perman` compiles as a `cdylib` and is injected into every login shell via `LD_PRELOAD=/lib/libperman.so` (set in `/etc/profile` by the container build). It intercepts `chdir` calls via a `#[no_mangle] extern "C" fn chdir` and validates the target path against the calling user's `allowed_dirs` list by querying the `userman` daemon. This enforces filesystem sandboxing without kernel modifications.

Key source files:

- `crates/user/perman/src/lib.rs` — `chdir` intercept and `userman` API call

### gpuman

`gpuman` detects GPUs and neural processing units at boot via sysfs and launches vendor-specific driver/runtime containers. It uses symlink polymorphism:

| Basename | Mode |
|----------|------|
| `gpuman` | Daemon — detect accelerators via sysfs, launch containers |
| `gpuctl` | CLI — query detected devices |

At startup, `gpuman` scans `/sys/class/drm/card*` (GPUs) and `/sys/class/accel/accel*` (NPUs), identifies vendors by PCI ID, and for each known vendor (NVIDIA, AMD, Intel) builds a container specification with the appropriate runtime image and device node bind-mounts.

Container images are configurable via kernel command-line parameters:

| Parameter | Default | Stack |
|-----------|---------|-------|
| `gpu_nvidia_image=` | `nvidia/cuda:latest` | CUDA |
| `gpu_amd_image=` | ROCm base image | ROCm |
| `gpu_intel_image=` | oneAPI base image | oneAPI |

Key source files:

- `crates/gpuman/src/main.rs` — entry point and mode dispatch
- `crates/gpuman/src/detect.rs` — sysfs scanning, `GpuVendor`, `DeviceClass`, `GpuDevice`
- `crates/gpuman/src/container.rs` — `build_container_spec()` construction
- `crates/gpuman/src/mode.rs` — `ModeOfOperation` enum

### pakman

`pakman` is a minimal package manager that leverages `nerdctl` and a NixOS base image to install arbitrary programs into the running system without modifying the read-only initramfs. Programs are stored as saved container image tarballs on a persistent data drive.

```bash
# Install one or more programs (builds a NixOS container and saves it to /data/progs)
pakman --install curl git

# Remove an installed program
pakman --remove curl

# Load and run an installed program interactively
pakman --run git
```

#### Install flow

```
pakman --install <pkg>
    ├─ Read kernel cmdline for data_drive= parameter
    ├─ Mount data_drive → /data  (if not already mounted)
    ├─ mkdir -p /data/progs
    └─ For each package (parallel threads):
           ├─ Write Dockerfile to $TMPDIR/<pkg>
           │     FROM nixos/nix
           │     ENTRYPOINT nix-shell -p <pkg> --run <pkg>
           ├─ nerdctl build -t local/<pkg>
           └─ nerdctl save local/<pkg> -o /data/progs/<pkg>.tar
```

#### Run flow

```
pakman --run <prog>
    ├─ WalkDir /data/progs — find <prog>.tar
    ├─ nerdctl load -i /data/progs/<prog>.tar
    └─ nerdctl run -it localhost/local/<prog>
```

#### Requirements

- `nerdctl` must be on `$PATH` (provided by the initramfs image).
- A `data_drive=<device>` entry must be present in `/proc/cmdline` for install/remove operations.
- The data drive must be a writable block device; it is mounted at `/data` if not already present in `/proc/mounts`.

Key source files:

- `crates/pakman/src/main.rs` — `clap` CLI entry point; dispatches `--install`, `--remove`, `--run`
- `crates/pakman/src/install.rs` — `PackageInstallation` — mounts the data drive, builds NixOS container images in parallel threads, saves them as tarballs
- `crates/pakman/src/run.rs` — `ProgRunner` — loads a saved tarball with `nerdctl load` and runs the container interactively

---

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

Smoke tests and micro-benchmarks for the core logic of all crates. Each crate gets its own `[[test]]` target under `crates/bench/benches/`. Run with:

```bash
cargo test -p bench
```

Coverage by target:

| Target | What is exercised |
|--------|-------------------|
| `actman` | `CmdLineOptions` parsing, `RebootCMD` dispatch |
| `cluman` | `IpRange` parsing/expansion, `CluManSchema`, `Tasks` (priority-aware), `ServerState`, `Mode` conversions, `Task` serde, `NicClass`, `NetworkInterface`, NIC detection, VLAN management |
| `dhcman` | DORA message construction, netconf helpers |
| `updman` | `UpdMan` schema parsing, `image_ref` construction |
| `pakman` | `PackageInstallation` queue, Dockerfile template rendering, `WalkDir` scan, `nerdctl` command construction |
| `gpuman` | `ModeOfOperation` dispatch, `GpuVendor`/`DeviceClass` formatting, `vendors_present` deduplication, `build_container_spec` construction and cmdline overrides |
| `isoman` | `build_gsi_fastboot` / `build_gsi_odin` end-to-end (boot image header validation, Odin MD5 trailer, monotonic size scaling), `MkbootimgParams` construction, `resolve_output` |

---

## Error Handling and Logging

All fallible operations return `miette::Result<()>` using the `IntoDiagnostic` trait. Structured logging uses the `tracing` crate (`info!`, `warn!`, `error!`) initialised via `tracing-subscriber`.

## Container Build

The multi-stage Containerfile (embedded in `isoman`) produces `os-<mode>.initramfs.tar.gz`:

| Stage | Base | Purpose |
|-------|------|---------|
| `stage0` | `alpine:latest` | Downloads `busybox-static` and the latest `nerdctl` full bundle; builds the target filesystem tree under `out/`, including `out/lib/` for shared libraries |
| `util` | `rust:alpine` | Compiles `actman`, `updman`, `dhcman`, `cluman`, `userman`, `gpuman`, and `libperman.so` for `x86_64-unknown-linux-musl`. Static binaries use the default Rust musl linker; `perman` is linked as a `cdylib` |
| `stage1` | `alpine:latest` | Assembles the final filesystem, writes init scripts, copies `libperman.so` to `out/lib/`, sets `LD_PRELOAD` in `/etc/profile`, installs the mode-selected `cluman` symlink under `/etc/init/start/`, and packs everything into a newc cpio archive |
| *(final)* | `scratch` | Exports `os.tar.gz` as `os.initramfs.tar.gz` |

The `MODE` build argument controls which `cluman` symlink (`client` or `server`) is installed under `/etc/init/start/`, making the cluster role a build-time decision baked into the image. `isoman` bakes this in automatically — no `--build-arg` is needed on the command line.

Init scripts installed at boot time:

| Path | Source | Purpose |
|------|--------|---------|
| `/etc/init/start/00-loopback` | inline shell | Brings up `lo` with `127.0.0.1/8` |
| `/etc/init/start/00-eth0` | symlink → `dhcman` | DHCP on `eth0` |
| `/etc/init/start/01-vlans` | inline shell | Creates VLAN interfaces from cmdline params (`vlan_base=`, `vlan_ids=`) |
| `/etc/init/start/login` | symlink → `userman` | Interactive login screen |
| `/etc/init/start/usersvc-local` | symlink → `userman` | Local user management daemon |
| `/etc/init/start/buildkitd` | symlink → `buildkitd` | BuildKit daemon for container builds |
| `/etc/init/start/containerd` | symlink → `containerd` | containerd runtime |
| `/etc/init/start/sshd` | symlink → `cluman` | SSH daemon |
| `/etc/init/start/gpuman` | symlink → `gpuman` | GPU/NPU accelerator manager |
| `/etc/init/start/sh` | symlink → `sh` | Fallback shell |
| `/etc/init/start/<mode>` | symlink → `cluman` | `cluman` in the baked-in mode |

---

## Documentation

The full documentation is built with [Docusaurus](https://docusaurus.io/) + [Aceternity UI](https://ui.aceternity.com/).

```bash
# Install dependencies
just docs-install

# Start dev server with hot reload
just docs-dev

# Build for production
just docs-build
```

The documentation is served via GitLab Pages at `https://losos.gitlab.io/losos-linux/docs/`.

## AI Disclosure

This project uses generative AI (Claude Sonnet 4.6) to generate documentation and tests. It is also used for refactoring and regression fighting and will be used in the future to review MRs and manage issues.