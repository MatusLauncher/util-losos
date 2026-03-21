# updman — Update Manager

`updman` performs an over-the-air update of the initramfs on the `BOOT` partition.
It reads a configuration file, pulls the new OS image from a container registry,
extracts the nested `os.initramfs.tar.gz`, and replaces the file on the boot partition.

## Update flow

```
/etc/update.json
    └─ UpdMan { base_url, image_tag, hash }
           ├─ nerdctl save <base_url>/<image_tag>  → dl.tar
           ├─ tar -xvf dl.tar  → $TMPDIR/out/
           ├─ tar -xvf <layer>.tar  → os.initramfs.tar.gz
           ├─ mount /dev/disk/by-label/BOOT → $TMPDIR/mnt/
           ├─ mv os.initramfs.tar.gz → $TMPDIR/mnt/os.initramfs.tar.gz
           └─ umount $TMPDIR/mnt/
```

## Configuration — `/etc/update.json`

```json
{
  "base_url":   "registry.example.com/mtos-v2",
  "image_tag":  "util-mdl:latest",
  "hash":       "<expected digest>"
}
```

| Field       | Description                                            |
|-------------|--------------------------------------------------------|
| `base_url`  | Container registry prefix (no trailing slash)          |
| `image_tag` | Image name and tag to pull with `nerdctl save`         |
| `hash`      | Reserved for future integrity verification             |

## Requirements

- `nerdctl` must be on `$PATH` (provided by the initramfs image).
- The block device `/dev/disk/by-label/BOOT` must exist and be writable.
- The update takes effect on the **next boot** — `updman` does not reboot the system.
