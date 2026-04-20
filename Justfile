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
#   just setup-sbctl         Generate Secure Boot key hierarchy (PK/KEK/db) via sbctl in an ephemeral Arch Linux container
# ── Configurable variables (override via env or 'just var=value recipe') ──────

kernel := env("KERNEL", "")
memory := env("MEMORY", "2G")
cpus := env("CPUS", "4")
kvm := env("KVM", "1")
disk := env("DISK", "")
output := env("OUTPUT", "os-client.iso")
ovmf_code := env("OVMF_CODE", "/usr/share/edk2/x64/OVMF_CODE.4m.fd")
ovmf_vars := env("OVMF_VARS", "/usr/share/edk2/x64/OVMF_VARS.4m.fd")
isoman_config := env("ISOMAN_CONFIG", "")

# ── Private helpers ───────────────────────────────────────────────────────────

# Load dm-integrity and ensure root access for cryptsetup loop-device operations.
_dm-integrity:
    sudo modprobe dm-integrity

# Generate Secure Boot keys only when sb-key.pem / sb-cert.pem are absent.
_ensure-sb-keys:
    #!/usr/bin/env bash
    set -euo pipefail
    if [[ ! -f sb-key.pem || ! -f sb-cert.pem ]]; then
        echo "==> Secure Boot keys not found — generating via sbctl..."
        just setup-sbctl
    fi

# ── Public recipes ────────────────────────────────────────────────────────────

# Pull all submodules to their latest remote commit and build the workspace
dev:
    @echo "==> Pulling submodules..."
    git submodule update --remote --merge

# Generate a full UEFI Secure Boot key hierarchy (PK, KEK, db) via sbctl in an
# ephemeral Arch Linux container.  The db signing key/cert are copied to
# sb-key.pem / sb-cert.pem in the project root where isoman --secure-boot
# picks them up automatically.  The full hierarchy lives in secure-boot/.
setup-sbctl:
    #!/usr/bin/env bash
    set -euo pipefail

    SB_DIR="$(pwd)/secure-boot"
    mkdir -p "$SB_DIR"

    echo "==> Generating Secure Boot key hierarchy via sbctl (ephemeral archlinux container)..."
    podman run --rm \
        --name sbctl-setup \
        -v "$SB_DIR:/usr/share/secureboot:Z" \
        docker.io/archlinux:latest \
        bash -c "
            pacman -Sy --noconfirm --quiet sbctl 2>&1 | tail -3
            sbctl create-keys
        "

    # The db key/cert are what sbsign (isoman --secure-boot) needs.
    cp "$SB_DIR/keys/db/db.key" sb-key.pem
    cp "$SB_DIR/keys/db/db.pem" sb-cert.pem
    chmod 600 sb-key.pem

    echo ""
    echo "==> Full key hierarchy written to secure-boot/"
    echo "    PK  : secure-boot/keys/PK/PK.{key,pem}"
    echo "    KEK : secure-boot/keys/KEK/KEK.{key,pem}"
    echo "    db  : secure-boot/keys/db/db.{key,pem}"
    echo ""
    echo "==> Signing key/cert copied to sb-key.pem / sb-cert.pem"
    echo "    Run 'just build-secure-boot' to sign the EFI binary with these keys."

# Launch initramfs in QEMU (default)
default: build-secure-boot run

# Build the OS ISO image (Hybrid BIOS+UEFI with LUKS2-encrypted initramfs partition)
build: _dm-integrity
    @echo "==> Building OS ISO image (LUKS2-encrypted P3)..."
    cargo run -p isoman -- --build --output "{{ output }}"
    @echo "==> ISO image written to {{ output }}"

# Build using a JSON config file (ISOMAN_CONFIG env or explicit path)
build-config config_path=isoman_config: _dm-integrity
    @echo "==> Building from config: {{ config_path }}"
    cargo run -p isoman -- --build --config "{{ config_path }}" --output "{{ output }}"

# Build a GSI (Fastboot + Odin) instead of a bootable ISO
build-gsi:
    @echo "==> Building GSI (Fastboot + Odin)..."
    cargo run -p isoman -- --build --gsi

# Build a Fastboot-only GSI boot.img
build-gsi-fastboot:
    cargo run -p isoman -- --build --gsi --gsi-fastboot

# Build production-hardened OS image (loglevel=0 + security mitigations baked into UKI cmdline)
build-prod: _dm-integrity _ensure-sb-keys
    @echo "==> Building production OS disk image (hardened cmdline)..."
    cargo run -p isoman -- --build --profile prod --kernel "{{ kernel }}"
    @echo "==> Production disk image written to os-<mode>.img"

# Build production live OS image (hardened cmdline + container-ready for preflight)
build-prod-live: _dm-integrity _ensure-sb-keys
    @echo "==> Building production live OS disk image (container-ready for preflight)..."
    cargo run -p isoman -- --build --profile prod-live --kernel "{{ kernel }}"
    @echo "==> Production live disk image written to os-<mode>.img"

# Build with Secure Boot signing (auto-generates sb-key.pem / sb-cert.pem if absent)
build-secure-boot: _dm-integrity _ensure-sb-keys
    @echo "==> Building with Secure Boot signing..."
    kernel_arg=$([[ -n "{{ kernel }}" ]] && echo "--kernel {{ kernel }}" || echo ""); \
    cargo run -p isoman -- --build --output "{{ output }}" --secure-boot true $kernel_arg

# Build initramfs then launch in QEMU
build-run: build run

# Build initramfs then run integration tests
build-test: build test

# Run testman integration tests (builds non-prod encrypted disk image first)
test: build
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

# Launch OS ISO image in QEMU via UEFI (OVMF pflash + virtio drive)
run:
    #!/usr/bin/env bash
    set -euo pipefail

    echo "==> Launching OS ISO image (UEFI)"
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
    [[ -n "{{ disk }}" ]] && data_disk_args=(-drive "file={{ disk }},format=raw,if=virtio")

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

# Launch OS ISO image in QEMU via Legacy BIOS
run-bios:
    #!/usr/bin/env bash
    set -euo pipefail

    echo "==> Launching OS ISO image (Legacy BIOS)"
    echo "    Image:     {{ output }}"
    echo "    Memory:    {{ memory }}"
    echo "    CPUs:      {{ cpus }}"
    [[ -n "{{ disk }}" ]] && echo "    Data disk: {{ disk }} (→ /dev/vdb)"
    echo ""

    kvm_flag=()
    [[ "{{ kvm }}" -eq 1 ]] && kvm_flag=(-enable-kvm)

    data_disk_args=()
    [[ -n "{{ disk }}" ]] && data_disk_args=(-drive "file={{ disk }},format=raw,if=virtio")

    exec qemu-system-x86_64 \
        -m "{{ memory }}" \
        -smp "{{ cpus }}" \
        -drive "file={{ output }},format=raw,if=virtio" \
        -nographic \
        -nic user \
        -netdev user,id=n0 \
        -device virtio-net-pci,netdev=n0 \
        "${data_disk_args[@]}" \
        "${kvm_flag[@]}"
