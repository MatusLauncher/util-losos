# util-mdl — build, launch, and test the initramfs OS.
#
# Recipes:
#   just build               Build initramfs from Containerfile
#   just build-config        Build from a JSON config file (ISOMAN_CONFIG)
#   just build-gsi           Build a GSI (Fastboot + Odin)
#   just build-gsi-fastboot  Build a Fastboot-only GSI boot.img
#   just build-secure-boot   Build with Secure Boot signing
#   just build-encrypted     Build with encrypted boot partition
#   just run                 Launch in QEMU (default)
#   just test                Run testman integration tests
#   just build-run           Build then launch
#   just build-test          Build then test

# ── Configurable variables (override via env or 'just var=value recipe') ──────

kernel       := env("KERNEL",       `find /boot -maxdepth 4 -type f -name "vmlinuz-$(uname -r)" -print -quit 2>/dev/null || true`)
memory       := env("MEMORY",       "2G")
cpus         := env("CPUS",         "4")
kvm          := env("KVM",          "1")
disk         := env("DISK",         "")
append       := env("APPEND",       "")
output       := env("OUTPUT",       "os.iso")
ovmf_code    := env("OVMF_CODE",    "/usr/share/edk2/ovmf/OVMF_CODE.fd")
ovmf_vars    := env("OVMF_VARS",    "/usr/share/edk2/ovmf/OVMF_VARS.fd")
isoman_config := env("ISOMAN_CONFIG", "")

# ── Public recipes ────────────────────────────────────────────────────────────

# Launch initramfs in QEMU (default)
default: run

# Build the initramfs image from the Containerfile (caching always enabled)
build:
    @echo "==> Building initramfs..."
    cargo run --manifest-path crates/isoman/Cargo.toml -- --build --output os.iso
    @echo "==> Extracting kernel and initramfs from ISO..."
    7z x -y os.iso boot/vmlinuz boot/initramfs.gz >/dev/null 2>&1 && mv boot/vmlinuz vmlinuz && mv boot/initramfs.gz initramfs.gz && rm -rf boot || true
    @echo "==> Initramfs written to os-<mode>.initramfs.tar.gz"

# Build using a JSON config file (ISOMAN_CONFIG env or explicit path)
build-config config_path=isoman_config:
    @echo "==> Building from config: {{config_path}}"
    cargo run --manifest-path crates/isoman/Cargo.toml -- --build --config "{{config_path}}"

# Build a GSI (Fastboot + Odin) instead of a bootable ISO
build-gsi:
    @echo "==> Building GSI (Fastboot + Odin)..."
    cargo run --manifest-path crates/isoman/Cargo.toml -- --build --gsi

# Build a Fastboot-only GSI boot.img
build-gsi-fastboot:
    cargo run --manifest-path crates/isoman/Cargo.toml -- --build --gsi --gsi-fastboot

# Build with Secure Boot signing (auto-generates sb-key.pem / sb-cert.pem if absent)
build-secure-boot:
    @echo "==> Building with Secure Boot signing..."
    cargo run --manifest-path crates/isoman/Cargo.toml -- --build --output "{{output}}" --secure-boot

# Build with encrypted boot partition (two-stage initramfs)
build-encrypted:
    @echo "==> Building with encrypted boot partition..."
    cargo run --manifest-path crates/isoman/Cargo.toml -- --build --output "{{output}}" --encrypt-boot

# Build initramfs then launch in QEMU
build-run: build run

# Build initramfs then run integration tests
build-test: build test

# Run testman integration tests
test:
    #!/usr/bin/env bash
    set -euo pipefail

    echo "==> Running testman integration tests"
    echo "    ISO:         {{output}}"
    echo "    Memory:      {{memory}}"
    echo "    CPUs:        {{cpus}}"
    echo ""

    exec env \
        ISO="{{output}}" \
        MEMORY="{{memory}}" \
        CPUS="{{cpus}}" \
        KVM="{{kvm}}" \
        cargo test --manifest-path crates/testman/Cargo.toml -- --test-threads=1 --include-ignored

# Launch initramfs in QEMU (using direct kernel+initramfs boot since ISO is UEFI-only)
run:
    #!/usr/bin/env bash
    set -euo pipefail

    echo "==> Launching initramfs"
    echo "    ISO:       {{output}}"
    echo "    Memory:    {{memory}}"
    echo "    CPUs:      {{cpus}}"
    [[ -n "{{disk}}" ]] && echo "    Disk:      {{disk}} (→ /dev/vda)"
    echo ""

    kvm_flag=()
    [[ "{{kvm}}" -eq 1 ]] && kvm_flag=(-enable-kvm)

    disk_args=()
    [[ -n "{{disk}}" ]] && disk_args=(-drive "file={{disk}},format=raw,if=virtio")

    # Prefer direct kernel+initramfs boot; fall back to UEFI ISO boot via OVMF.
    if [[ -f vmlinuz && -f initramfs.gz ]]; then
        kernel_args=(-kernel vmlinuz -initrd initramfs.gz -append "console=ttyS0 earlyprintk=ttyS0 net.ifnames=0 biosdevname=0")
    else
        # ISO is UEFI-only — load OVMF firmware via pflash so QEMU can boot it.
        # OVMF_VARS must be writable (firmware writes EFI variables at runtime);
        # use a temp copy to avoid mutating the system-wide file.
        tmp_vars=$(mktemp --suffix=.fd)
        cp "{{ovmf_vars}}" "$tmp_vars"
        trap "rm -f $tmp_vars" EXIT
        kernel_args=(
            -drive "if=pflash,format=raw,readonly=on,file={{ovmf_code}}"
            -drive "if=pflash,format=raw,file=$tmp_vars"
            -cdrom "{{output}}"
        )
    fi

    exec qemu-system-x86_64 \
        -m "{{memory}}" \
        -smp "{{cpus}}" \
        "${kernel_args[@]}" \
        -nographic \
        -nic user \
        -netdev user,id=n0 \
        -device virtio-net-pci,netdev=n0 \
        "${disk_args[@]}" \
        "${kvm_flag[@]}"

# ── Documentation (Docusaurus + Aceternity UI) ────────────────────────────────

# Install documentation dependencies
docs-install:
    cd book && bun install

# Start documentation dev server with hot reload
docs-dev:
    cd book && bun run start

# Build documentation for production
docs-build:
    cd book && bun run build

# Serve built documentation locally
docs-serve:
    cd book && bun run serve

# Build and serve documentation
docs-build-serve: docs-build docs-serve

