# util-mdl — build, launch, and test the initramfs OS.
#
# Recipes:
#   just build               Build OS disk image (LUKS2-encrypted P3, default)
#   just build-config        Build from a JSON config file (ISOMAN_CONFIG)
#   just build-gsi           Build a GSI (Fastboot + Odin)
#   just build-gsi-fastboot  Build a Fastboot-only GSI boot.img
#   just build-secure-boot   Build with Secure Boot signing
#   just build-prod          Build production-hardened image (loglevel=0 + mitigations)
#   just build-prod-live     Build production live image (hardened + container-ready for preflight)
#   just run                 Launch in QEMU (UEFI via OVMF)
#   just test                Run testman integration tests
#   just build-run           Build then launch
#   just build-test          Build then test
#   just dev                 Pull all submodules and build the workspace
# ── Configurable variables (override via env or 'just var=value recipe') ──────

kernel := env("KERNEL", "")
memory := env("MEMORY", "2G")
cpus := env("CPUS", "4")
kvm := env("KVM", "1")
disk := env("DISK", "")
output := env("OUTPUT", "os-client.img")
ovmf_code := env("OVMF_CODE", "/usr/share/edk2/ovmf/OVMF_CODE.fd")
ovmf_vars := env("OVMF_VARS", "/usr/share/edk2/ovmf/OVMF_VARS.fd")
isoman_config := env("ISOMAN_CONFIG", "")

# ── Public recipes ────────────────────────────────────────────────────────────

# Pull all submodules to their latest remote commit and build the workspace
dev:
    @echo "==> Pulling submodules..."
    git submodule update --remote --merge

# Launch initramfs in QEMU (default)
default: build-secure-boot run

# Build the OS disk image (GPT with LUKS2-encrypted initramfs partition)
build:
    @echo "==> Building OS disk image (LUKS2-encrypted P3)..."
    cargo run -p isoman -- --build
    @echo "==> Disk image written to os-<mode>.img"

# Build using a JSON config file (ISOMAN_CONFIG env or explicit path)
build-config config_path=isoman_config:
    @echo "==> Building from config: {{ config_path }}"
    cargo run -p isoman -- --build --config "{{ config_path }}"

# Build a GSI (Fastboot + Odin) instead of a bootable ISO
build-gsi:
    @echo "==> Building GSI (Fastboot + Odin)..."
    cargo run -p isoman -- --build --gsi

# Build a Fastboot-only GSI boot.img
build-gsi-fastboot:
    cargo run -p isoman -- --build --gsi --gsi-fastboot

# Build production-hardened OS image (loglevel=0 + security mitigations baked into UKI cmdline)
build-prod:
    @echo "==> Building production OS disk image (hardened cmdline)..."
    cargo run -p isoman -- --build --profile prod --kernel "{{ kernel }}"
    @echo "==> Production disk image written to os-<mode>.img"

# Build production live OS image (hardened cmdline + container-ready for preflight)
build-prod-live:
    @echo "==> Building production live OS disk image (container-ready for preflight)..."
    cargo run -p isoman -- --build --profile prod-live --kernel "{{ kernel }}"
    @echo "==> Production live disk image written to os-<mode>.img"

# Build with Secure Boot signing (auto-generates sb-key.pem / sb-cert.pem if absent)
build-secure-boot:
    @echo "==> Building with Secure Boot signing..."
    cargo run -p isoman -- --build --output "{{ output }}" --secure-boot --kernel "{{ kernel }}"

# Build initramfs then launch in QEMU
build-run: build run

# Build initramfs then run integration tests
build-test: build test

# Run testman integration tests
test:
    #!/usr/bin/env bash
    set -euo pipefail

    echo "==> Running testman integration tests"
    echo "    ISO:         {{ output }}"
    echo "    Memory:      {{ memory }}"
    echo "    CPUs:        {{ cpus }}"
    echo ""

    exec env \
        ISO="{{ output }}" \
        MEMORY="{{ memory }}" \
        CPUS="{{ cpus }}" \
        KVM="{{ kvm }}" \
        cargo test --manifest-path crates/testman/Cargo.toml -- --test-threads=1 --include-ignored

# Launch OS disk image in QEMU via UEFI (OVMF pflash + virtio drive)
run:
    #!/usr/bin/env bash
    set -euo pipefail

    echo "==> Launching OS disk image"
    echo "    Image:     {{ output }}"
    echo "    Memory:    {{ memory }}"
    echo "    CPUs:      {{ cpus }}"
    [[ -n "{{ disk }}" ]] && echo "    Data disk: {{ disk }} (→ /dev/vdb)"
    echo ""

    kvm_flag=()
    [[ "{{ kvm }}" -eq 1 ]] && kvm_flag=(-enable-kvm)

    # Extra data disk (e.g. for persistent storage).  The OS image is /dev/vda;
    # the optional data disk appears as /dev/vdb.
    data_disk_args=()
    [[ -n "{{ disk }}" ]] && data_disk_args=(-drive "file={{ disk }},format=raw,if=virtio,${data_disk_args[@]}")

    # OVMF_VARS must be writable; use a temp copy to avoid mutating the
    # system-wide file.
    tmp_vars=$(mktemp --suffix=.fd)
    cp "{{ ovmf_vars }}" "$tmp_vars"
    trap "rm -f $tmp_vars" EXIT

    exec qemu-system-x86_64 \
        -m "{{ memory }}" \
        -smp "{{ cpus }}" \
        -drive "if=pflash,format=raw,readonly=on,file={{ ovmf_code }}" \
        -drive "if=pflash,format=raw,file=$tmp_vars" \
        -drive "file={{ output }},format=raw,if=virtio" \
        -nographic \
        -nic user \
        -netdev user,id=n0 \
        -device virtio-net-pci,netdev=n0 \
        "${data_disk_args[@]}" \
        "${kvm_flag[@]}"
