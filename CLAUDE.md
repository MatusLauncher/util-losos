# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

`util-mdl` is a minimal, container-native OS init system and update manager packaged as a bootable initramfs. Each component lives in its own Git repository under the `mtos-v2` namespace and is linked here as a git submodule:

| Submodule path | Repository | Description |
|---|---|---|
| `crates/actman` | [mtos-v2/actman](https://gitlab.com/mtos-v2/actman) | Init system (PID 1) |
| `crates/updman` | [mtos-v2/updman](https://gitlab.com/mtos-v2/updman) | Update manager |
| `crates/pakman` | [mtos-v2/pakman](https://gitlab.com/mtos-v2/pakman) | Package manager |
| `crates/cluman` | [mtos-v2/cluman](https://gitlab.com/mtos-v2/cluman) | Cluster/container manager |
| `crates/isoman` | [mtos-v2/isoman](https://gitlab.com/mtos-v2/isoman) | ISO/initramfs builder |
| `crates/dhcman` | [mtos-v2/dhcman](https://gitlab.com/mtos-v2/dhcman) | DHCP client |
| `crates/testman` | [mtos-v2/testman](https://gitlab.com/mtos-v2/testman) | Integration test harness |
| `crates/user` | [mtos-v2/userman](https://gitlab.com/mtos-v2/userman) | User management (userman + perman) |
| `crates/sshman` | [mtos-v2/sshman](https://gitlab.com/mtos-v2/sshman) | SSH server |
| `book` | [mtos-v2/docs](https://gitlab.com/mtos-v2/docs) | Documentation |

The `crates/bench` directory is the only in-tree crate — it contains integration benchmarks that depend on all other crates via git dependencies.

## Getting Started

```bash
# Clone with all submodules
git clone --recurse-submodules https://gitlab.com/mtos-v2/util-mdl.git

# Or initialize submodules after cloning
git submodule update --init --recursive
```

## Build Commands

```bash
# Build bench (the only workspace member)
cargo build

# Build a specific submodule crate
cargo build --manifest-path crates/actman/Cargo.toml

# Release build
cargo build --release --manifest-path crates/actman/Cargo.toml

# Static MUSL target (used in container/initramfs)
cargo build --release --target x86_64-unknown-linux-musl --manifest-path crates/actman/Cargo.toml

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
# Run tests against a pre-built initramfs
./launch.sh --test

# Build initramfs first, then run tests
./launch.sh --build --test

# Disable KVM for CI environments
KVM=0 ./launch.sh --test
```

`testman` respects env vars: `KERNEL`, `INITRAMFS`, `MEMORY` (default `2G`), `CPUS` (default `2`), `KVM` (default `1`).

## Architecture

### Submodule Structure

Each crate is an independent Git repository. Inter-crate dependencies use git URLs:

```
actman (core library, no deps)
  ├─→ updman
  ├─→ pakman
  ├─→ cluman
  │   └─→ isoman
  └─→ userman (also depends on perman via path within same repo)

sshman (depends on actman + userman)
dhcman (standalone)
testman (standalone)
bench (in-tree, depends on all via git deps)
```

### actman — Init System

Boot path: kernel executes `/bin/init` (actman binary) → determines mode from `argv[0]`:

- **Init mode**: calls `Preboot::mount()` to set up filesystems, then walks `/etc/init/start/` and spawns each script in order.
- **PowerOff/Reboot mode**: walks `/etc/init/stop/` running shutdown scripts, then calls `rustix::system::reboot()` with the appropriate `RebootCommand`.

Symlinks created in the container image: `/bin/poweroff → /bin/init`, `/bin/reboot → /bin/init`.

### updman — Update Manager

Reads `/etc/update.json` (`base_url`, `image_tag`, `hash`) → runs `nerdctl save` to pull/export the container image → extracts the nested `os.initramfs.tar.gz` → mounts the `BOOT` partition and replaces the initramfs file.

### pakman — Package Manager

CLI tool for installing, removing, and running programs inside the initramfs environment. Uses NixOS container images built with `nerdctl` and stores the resulting tarballs on a persistent data drive.

### sshman — SSH Server

SSH daemon built on `russh`. Authenticates users against the userman HTTP daemon (password, SSH public key, TOTP/second-password 2FA via keyboard-interactive). Spawns PTY sessions with Landlock filesystem sandboxing. Auto-generates an Ed25519 host key at `/etc/ssh/host_key` on first boot. Uses symlink polymorphism: `sshman` or `sshd` → Daemon mode.

### Error Handling and Logging

All fallible operations return `miette::Result<()>` using `IntoDiagnostic` trait conversions. Structured logging via the `tracing` crate (`info!()` macro throughout).

### Container Build (Containerfile)

Multi-stage OCI build:
1. **stage0**: Alpine + busybox-static + nerdctl download
2. **util**: Rust + musl — compiles crates for `x86_64-unknown-linux-musl`
3. **stage1**: Assembles final filesystem hierarchy, creates cpio archive → `os.initramfs.tar.gz`

The final artifact (`os.initramfs.tar.gz`) is what gets deployed to the BOOT partition and loaded by the kernel as an initramfs.
