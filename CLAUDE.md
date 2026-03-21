# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

`util-mdl` is a minimal, container-native OS init system and update manager packaged as a bootable initramfs. It contains two Rust crates:

- **actman** — the init system (PID 1). A single binary that acts as `init`, `poweroff`, or `reboot` depending on its `argv[0]` basename (symlink polymorphism).
- **updman** — downloads a container image, extracts the nested initramfs tarball, and swaps it onto the BOOT partition for next boot.

## Build Commands

```bash
# Standard debug build
cargo build

# Release build (production binaries)
cargo build --release

# Static MUSL target (used in container/initramfs)
cargo build --release --target x86_64-unknown-linux-musl

# Build the full initramfs container image
podman build --no-cache -t util-mdl-build .

# Launch in QEMU (requires pre-built initramfs + kernel)
./launch.sh

# Build initramfs then launch
./launch.sh --build
```

`launch.sh` respects env vars: `KERNEL` (path to vmlinuz), `MEMORY` (default `2G`), `CPUS` (default `2`).

## Lint and Test

```bash
cargo clippy
cargo fmt --check
```

Integration tests run via `testman` — boots the initramfs in QEMU and asserts expected log output:

```bash
# Build only (no QEMU needed)
cargo build -p testman

# Run tests against a pre-built initramfs
./launch.sh --test

# Build initramfs first, then run tests
./launch.sh --build --test

# Disable KVM for CI environments
KVM=0 ./launch.sh --test
```

`testman` respects env vars: `KERNEL`, `INITRAMFS`, `MEMORY` (default `2G`), `CPUS` (default `2`), `KVM` (default `1`).

## Architecture

### actman — Init System

Boot path: kernel executes `/bin/init` (actman binary) → determines mode from `argv[0]`:

- **Init mode**: calls `Preboot::mount()` to set up filesystems, then walks `/etc/init/start/` and spawns each script in order.
- **PowerOff/Reboot mode**: walks `/etc/init/stop/` running shutdown scripts, then calls `rustix::system::reboot()` with the appropriate `RebootCommand`.

Key files:
- `crates/actman/src/main.rs` — entry point and mode dispatch
- `crates/actman/src/reboot.rs` — `RebootCMD` enum mapping binary name → syscall
- `crates/actman/src/preboot.rs` — filesystem mounting logic
- `crates/actman/src/cmdline.rs` — kernel command-line parser

Symlinks created in the container image: `/bin/poweroff → /bin/init`, `/bin/reboot → /bin/init`.

### updman — Update Manager

Reads `/etc/update.json` (`base_url`, `image_tag`, `hash`) → runs `nerdctl save` to pull/export the container image → extracts the nested `os.initramfs.tar.gz` → mounts the `BOOT` partition and replaces the initramfs file.

Key files:
- `crates/updman/src/main.rs` — entry point
- `crates/updman/src/schema.rs` — `UpdMan` struct and `update()` orchestration

### Error Handling and Logging

All fallible operations return `miette::Result<()>` using `IntoDiagnostic` trait conversions. Structured logging via the `tracing` crate (`info!()` macro throughout).

### Container Build (Containerfile)

Multi-stage OCI build:
1. **stage0**: Alpine + busybox-static + nerdctl download
2. **util**: Rust + musl — compiles both crates for `x86_64-unknown-linux-musl`
3. **stage1**: Assembles final filesystem hierarchy, creates cpio archive → `os.initramfs.tar.gz`

The final artifact (`os.initramfs.tar.gz`) is what gets deployed to the BOOT partition and loaded by the kernel as an initramfs.
