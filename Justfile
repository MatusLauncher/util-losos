# util-mdl — build, launch, and test the initramfs OS.
#
# Recipes:
#   just build        Build initramfs from Containerfile
#   just run          Launch in QEMU (default)
#   just test         Run testman integration tests
#   just iso          Create bootable ISO
#   just build-run    Build then launch
#   just build-test   Build then test

# ── Configurable variables (override via env or 'just var=value recipe') ──────

kernel    := env_var_or_default("KERNEL",    `find /boot -maxdepth 4 -type f -name "vmlinuz-$(uname -r)" -print -quit 2>/dev/null`)
initramfs := env_var_or_default("INITRAMFS", `find . -maxdepth 2 -name "*.initramfs.tar.gz" -print -quit 2>/dev/null`)
memory    := env_var_or_default("MEMORY",    "2G")
cpus      := env_var_or_default("CPUS",      "2")
kvm       := env_var_or_default("KVM",       "1")
disk      := env_var_or_default("DISK",      "")
append    := env_var_or_default("APPEND",    "")
output    := env_var_or_default("OUTPUT",    "os.iso")

# Path where the supplemental kernel-module initrd is written.
# Named by kernel version so stale files from old kernels don't interfere.
_mods_path := "/tmp/util-mdl-mods-" + `uname -r` + ".initrd"

# ── Public recipes ────────────────────────────────────────────────────────────

# Launch initramfs in QEMU (default)
default: run

# Build the initramfs image from the Containerfile
build:
    @echo "==> Building initramfs..."
    podman build --no-cache -t util-mdl-build .
    podman create --name util-mdl-export util-mdl-build
    podman cp util-mdl-export:/os.initramfs.tar.gz os.initramfs.tar.gz
    podman rm util-mdl-export
    @echo "==> Initramfs written to os.initramfs.tar.gz"

# Build initramfs then launch in QEMU
build-run: build run

# Build initramfs then run integration tests
build-test: build test

# Launch initramfs in QEMU
run: _check _mods
    #!/usr/bin/env bash
    set -euo pipefail
    merged=$(mktemp --suffix=.initramfs)
    trap "rm -f $merged" EXIT
    cat "{{initramfs}}" "{{_mods_path}}" > "$merged"

    echo "==> Launching initramfs"
    echo "    Kernel:    {{kernel}}"
    echo "    Initramfs: {{initramfs}}"
    echo "    Memory:    {{memory}}"
    echo "    CPUs:      {{cpus}}"
    [[ -n "{{disk}}" ]] && echo "    Disk:      {{disk}} (→ /dev/vda)"
    echo ""

    kvm_flag=()
    [[ "{{kvm}}" -eq 1 ]] && kvm_flag=(-enable-kvm)

    disk_args=()
    [[ -n "{{disk}}" ]] && disk_args=(-drive "file={{disk}},format=raw,if=virtio")

    append_extra=""
    [[ -n "{{append}}" ]] && append_extra=" {{append}}"

    exec qemu-system-x86_64 \
        -kernel "{{kernel}}" \
        -initrd "$merged" \
        -append "quiet net.ifnames=0 biosdevname=0$append_extra" \
        -m "{{memory}}" \
        -smp "{{cpus}}" \
        -netdev user,id=n0 \
        -device virtio-net-pci,netdev=n0 \
        "${disk_args[@]}" \
        "${kvm_flag[@]}"

# Run testman integration tests
test: _check _mods
    #!/usr/bin/env bash
    set -euo pipefail
    merged=$(mktemp --suffix=.initramfs)
    trap "rm -f $merged" EXIT
    cat "{{initramfs}}" "{{_mods_path}}" > "$merged"

    echo "==> Running testman integration tests"
    echo "    Kernel:    {{kernel}}"
    echo "    Initramfs: $merged"
    echo "    Memory:    {{memory}}"
    echo "    CPUs:      {{cpus}}"
    echo ""

    exec env \
        KERNEL="{{kernel}}" \
        INITRAMFS="$merged" \
        MEMORY="{{memory}}" \
        CPUS="{{cpus}}" \
        KVM="{{kvm}}" \
        cargo test --manifest-path crates/testman/Cargo.toml -- --test-threads=1 --include-ignored

# Create a bootable ISO image
iso: _check
    #!/usr/bin/env bash
    set -euo pipefail
    echo "==> Creating bootable ISO"
    echo "    Kernel:    {{kernel}}"
    echo "    Initramfs: {{initramfs}}"
    echo "    Output:    {{output}}"
    echo ""
    exec env \
        KERNEL="{{kernel}}" \
        INITRAMFS="{{initramfs}}" \
        OUTPUT="{{output}}" \
        cargo run --manifest-path crates/isoman/Cargo.toml

# ── Private helpers ───────────────────────────────────────────────────────────

# Validate that kernel and initramfs exist before attempting a launch.
[private]
_check:
    #!/usr/bin/env bash
    set -euo pipefail
    if [[ ! -f "{{initramfs}}" ]]; then
        echo "error: initramfs not found — run 'just build' first" >&2
        exit 1
    fi
    if [[ ! -f "{{kernel}}" ]]; then
        echo "error: kernel not found at {{kernel}}" >&2
        echo "       set KERNEL= or pass kernel=<path> to just" >&2
        exit 1
    fi

# Build a supplemental initrd with virtio-net and dm-crypt kernel modules.
# Decompresses .ko.xz files from the host so busybox insmod can load them
# without xz support.  dm-crypt/dm-mod are included only when present as
# loadable modules (kernels with CONFIG_DM_CRYPT=y skip this step).
[private]
_mods:
    #!/usr/bin/env bash
    set -euo pipefail
    kver=$(uname -r)
    mods_root=$(mktemp -d)
    trap "rm -rf $mods_root" EXIT

    net_core="$mods_root/lib/modules/$kver/kernel/net/core"
    net_drv="$mods_root/lib/modules/$kver/kernel/drivers/net"
    dm_drv="$mods_root/lib/modules/$kver/kernel/drivers/md"
    mkdir -p "$net_core" "$net_drv" "$dm_drv" "$mods_root/etc/init/start"

    xz -dk --stdout "/lib/modules/$kver/kernel/net/core/failover.ko.xz"        > "$net_core/failover.ko"
    xz -dk --stdout "/lib/modules/$kver/kernel/drivers/net/net_failover.ko.xz" > "$net_drv/net_failover.ko"
    xz -dk --stdout "/lib/modules/$kver/kernel/drivers/net/virtio_net.ko.xz"   > "$net_drv/virtio_net.ko"

    printf '#!/bin/sh\ninsmod /lib/modules/%s/kernel/net/core/failover.ko\ninsmod /lib/modules/%s/kernel/drivers/net/net_failover.ko\ninsmod /lib/modules/%s/kernel/drivers/net/virtio_net.ko\n' \
        "$kver" "$kver" "$kver" > "$mods_root/etc/init/start/000-load-virtio"
    chmod +x "$mods_root/etc/init/start/000-load-virtio"

    dm_mod="/lib/modules/$kver/kernel/drivers/md/dm-mod.ko.xz"
    dm_crypt="/lib/modules/$kver/kernel/drivers/md/dm-crypt.ko.xz"
    if [[ -f "$dm_mod" && -f "$dm_crypt" ]]; then
        xz -dk --stdout "$dm_mod"   > "$dm_drv/dm-mod.ko"
        xz -dk --stdout "$dm_crypt" > "$dm_drv/dm-crypt.ko"
        printf '#!/bin/sh\ninsmod /lib/modules/%s/kernel/drivers/md/dm-mod.ko\ninsmod /lib/modules/%s/kernel/drivers/md/dm-crypt.ko\n' \
            "$kver" "$kver" > "$mods_root/etc/init/start/000-load-dm"
        chmod +x "$mods_root/etc/init/start/000-load-dm"
    fi

    (cd "$mods_root" && find . | cpio -o -H newc 2>/dev/null | gzip > "{{_mods_path}}")
