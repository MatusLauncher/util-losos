#!/usr/bin/env bash
set -Eeux pipefail

INITRAMFS="${INITRAMFS:-$(find . -name "*.initramfs.tar.gz")}"
KERNEL="${KERNEL:-$(find /boot -maxdepth 4 -type f -name "vmlinuz-$(uname -r)" -print -quit 2>/dev/null)}"
MEMORY="${MEMORY:-2G}"
CPUS="${CPUS:-2}"
# Optional raw disk image to attach as /dev/vda (useful for LUKS/LVM testing).
DISK="${DISK:-}"
# Extra kernel command-line parameters appended verbatim (e.g. luks_device=,lvm=1,nfs_mount=).
APPEND="${APPEND:-}"

build_initramfs() {
    echo "==> Building initramfs..."
    podman build --no-cache -t util-mdl-build .
    podman create --name util-mdl-export util-mdl-build
    podman cp util-mdl-export:/os.initramfs.tar.gz "$INITRAMFS"
    podman rm util-mdl-export
    echo "==> Initramfs written to $INITRAMFS"
}

usage() {
    echo "Usage: $0 [--build] [--test] [--iso] [--kernel <path>] [--disk <path>]"
    echo ""
    echo "Options:"
    echo "  --build          Build the initramfs from Containerfile before launching"
    echo "  --test           Run testman integration tests instead of interactive QEMU"
    echo "  --iso            Create a bootable ISO image from the kernel and initramfs"
    echo "  --kernel <path>  Path to kernel image (default: host kernel)"
    echo "  --disk <path>    Attach a raw disk image as /dev/vda (for LUKS/LVM testing)"
    echo ""
    echo "Environment variables:"
    echo "  KERNEL   Kernel image path (default: /boot/vmlinuz-\$(uname -r))"
    echo "  MEMORY   VM memory (default: 2G)"
    echo "  CPUS     VM CPU count (default: 2)"
    echo "  KVM      Enable KVM acceleration (default: 1, set to 0 for CI)"
    echo "  OUTPUT   ISO output path for --iso (default: os.iso)"
    echo "  DISK     Raw disk image path to attach as /dev/vda (same as --disk)"
    echo "  APPEND   Extra kernel cmdline params (e.g. 'luks_device=/dev/vda lvm=1')"
}

BUILD=0
TEST=0
ISO=0

while [[ $# -gt 0 ]]; do
    case "$1" in
        --build) BUILD=1; shift ;;
        --test) TEST=1; shift ;;
        --iso) ISO=1; shift ;;
        --kernel) KERNEL="$2"; shift 2 ;;
        --disk) DISK="$2"; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) echo "Unknown option: $1"; usage; exit 1 ;;
    esac
done

if [[ "$BUILD" -eq 1 ]]; then
    build_initramfs
fi

if [[ ! -f "$INITRAMFS" ]]; then
    echo "Error: $INITRAMFS not found. Run with --build to build it first."
    exit 1
fi

if [[ ! -f "$KERNEL" ]]; then
    echo "Error: kernel not found at $KERNEL"
    echo "Set KERNEL env var or pass --kernel <path>"
    exit 1
fi

# Build a supplemental initrd with virtio-net kernel modules so that the NIC
# driver is available even when the main initramfs has no modules.
_build_mods_initrd() {
    local kver="$1"
    local mods_root
    mods_root=$(mktemp -d)

    local net_core="$mods_root/lib/modules/$kver/kernel/net/core"
    local net_drivers="$mods_root/lib/modules/$kver/kernel/drivers/net"
    local dm_drivers="$mods_root/lib/modules/$kver/kernel/drivers/md"
    mkdir -p "$net_core" "$net_drivers" "$dm_drivers" "$mods_root/etc/init/start"

    # Decompress modules so busybox insmod doesn't need xz support.
    xz -dk --stdout "/lib/modules/$kver/kernel/net/core/failover.ko.xz" \
        > "$net_core/failover.ko"
    xz -dk --stdout "/lib/modules/$kver/kernel/drivers/net/net_failover.ko.xz" \
        > "$net_drivers/net_failover.ko"
    xz -dk --stdout "/lib/modules/$kver/kernel/drivers/net/virtio_net.ko.xz" \
        > "$net_drivers/virtio_net.ko"

    # Startup script (sorts before 00-eth0 so it runs first).
    cat > "$mods_root/etc/init/start/000-load-virtio" <<EOF
#!/bin/sh
insmod /lib/modules/$kver/kernel/net/core/failover.ko
insmod /lib/modules/$kver/kernel/drivers/net/net_failover.ko
insmod /lib/modules/$kver/kernel/drivers/net/virtio_net.ko
EOF
    chmod +x "$mods_root/etc/init/start/000-load-virtio"

    # Optionally include dm-mod and dm-crypt for LUKS/LVM testing.
    # Only added when the modules are present as loadable .ko.xz files;
    # if they are compiled into the kernel they don't need loading.
    local dm_mod_src="/lib/modules/$kver/kernel/drivers/md/dm-mod.ko.xz"
    local dm_crypt_src="/lib/modules/$kver/kernel/drivers/md/dm-crypt.ko.xz"
    if [[ -f "$dm_mod_src" && -f "$dm_crypt_src" ]]; then
        xz -dk --stdout "$dm_mod_src"   > "$dm_drivers/dm-mod.ko"
        xz -dk --stdout "$dm_crypt_src" > "$dm_drivers/dm-crypt.ko"
        cat > "$mods_root/etc/init/start/000-load-dm" <<EOF
#!/bin/sh
insmod /lib/modules/$kver/kernel/drivers/md/dm-mod.ko
insmod /lib/modules/$kver/kernel/drivers/md/dm-crypt.ko
EOF
        chmod +x "$mods_root/etc/init/start/000-load-dm"
    fi

    local out
    out=$(mktemp)
    (cd "$mods_root" && find . | cpio -o -H newc 2>/dev/null | gzip > "$out")
    rm -rf "$mods_root"
    echo "$out"
}

KVER=$(uname -r)
MODS_INITRD=$(_build_mods_initrd "$KVER")
MERGED_INITRAMFS=$(mktemp --suffix=.initramfs)
cat "$INITRAMFS" "$MODS_INITRD" > "$MERGED_INITRAMFS"
rm -f "$MODS_INITRD"
trap 'rm -f "$MERGED_INITRAMFS"' EXIT
INITRAMFS="$MERGED_INITRAMFS"

if [[ "$TEST" -eq 1 ]]; then
    echo "==> Running testman integration tests"
    echo "    Kernel:    $KERNEL"
    echo "    Initramfs: $INITRAMFS"
    echo "    Memory:    $MEMORY"
    echo "    CPUs:      $CPUS"
    echo ""
    exec env \
        KERNEL="$KERNEL" \
        INITRAMFS="$INITRAMFS" \
        MEMORY="$MEMORY" \
        CPUS="$CPUS" \
        KVM="${KVM:-1}" \
        cargo test --manifest-path crates/testman/Cargo.toml -- --test-threads=1 --include-ignored
fi

if [[ "$ISO" -eq 1 ]]; then
    echo "==> Creating bootable ISO"
    echo "    Kernel:    $KERNEL"
    echo "    Initramfs: $INITRAMFS"
    echo "    Output:    ${OUTPUT:-os.iso}"
    echo ""
    exec env \
        KERNEL="$KERNEL" \
        INITRAMFS="$INITRAMFS" \
        OUTPUT="${OUTPUT:-os.iso}" \
        cargo run --manifest-path crates/isoman/Cargo.toml
fi

echo "==> Launching initramfs"
echo "    Kernel:    $KERNEL"
echo "    Initramfs: $INITRAMFS"
echo "    Memory:    $MEMORY"
echo "    CPUs:      $CPUS"
echo ""

KVM_FLAG=()
if [[ "${KVM:-1}" -eq 1 ]]; then
    KVM_FLAG=(-enable-kvm)
fi

DISK_ARGS=()
if [[ -n "$DISK" ]]; then
    echo "    Disk:      $DISK (→ /dev/vda)"
    DISK_ARGS=(
        -drive "file=${DISK},format=raw,if=virtio"
    )
fi

exec qemu-system-x86_64 \
    -kernel "$KERNEL" \
    -initrd "$INITRAMFS" \
    -append "quiet net.ifnames=0 biosdevname=0${APPEND:+ }${APPEND}" \
    -m "$MEMORY" \
    -smp "$CPUS" \
    -netdev user,id=n0 \
    -device virtio-net-pci,netdev=n0 \
    "${DISK_ARGS[@]}" \
    "${KVM_FLAG[@]}"
