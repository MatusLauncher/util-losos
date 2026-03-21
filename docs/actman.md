# actman — Init System

`actman` is the PID 1 init binary for the `util-mdl` initramfs OS. A single compiled binary serves three roles depending on its `argv[0]` basename, selected at runtime via symlinks:

| Basename   | Role                                      |
|------------|-------------------------------------------|
| `init`     | System initialiser — mounts filesystems and spawns startup scripts |
| `poweroff` | Runs shutdown scripts then powers off the machine |
| `reboot`   | Runs shutdown scripts then reboots the machine |

## Boot sequence

```
kernel → execve /bin/init (actman)
    └─ argv[0] == "init"
           ├─ Preboot::mount()          mount pseudo-filesystems
           └─ WalkDir /etc/init/start/  spawn each script in order
```

## Shutdown sequence

```
poweroff / reboot
    ├─ WalkDir /etc/init/stop/          spawn each stop script in order
    └─ rustix::system::reboot()         RebootCommand::{PowerOff, Restart}
```

## Symlinks (created in Containerfile)

```
/bin/poweroff → /bin/init
/bin/reboot   → /bin/init
/init         → bin/init      (kernel looks here first)
```

## Kernel command line

`actman` parses `/proc/cmdline` via [`CmdLineOptions`] for future use — the parsed
key=value pairs are available but not yet acted upon by the init logic.

## Startup scripts

Any executable placed (or symlinked) under `/etc/init/start/` is spawned at boot.
Scripts under `/etc/init/stop/` are spawned during shutdown. Order follows
[`walkdir`](https://docs.rs/walkdir) directory traversal order (lexicographic).

Default symlinks created by the Containerfile:

```
/etc/init/start/udhcpc     → /bin/udhcpc
/etc/init/start/buildkitd  → /bin/buildkitd
/etc/init/start/containerd → /bin/containerd
/etc/init/start/sh         → /bin/sh
```
