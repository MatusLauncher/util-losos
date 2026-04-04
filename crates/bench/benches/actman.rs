//! Smoke tests for the `actman` crate.
//!
//! Exercises:
//! * `CmdLineOptions::param_search` — the kernel command-line parser, at
//!   various input sizes and shapes.
//! * `RebootCMD::from` — basename-to-mode dispatch for every variant plus the
//!   full-path and unknown-name fast-paths.
//! * `Preboot::new` / `Preboot::default` — construction (live sysfs probes).

use actman::{cmdline::CmdLineOptions, nfs::parse_nfs_spec, preboot::Preboot, reboot::RebootCMD};
use std::hint::black_box;

const CMDLINE_BARE_FLAGS: &str = "quiet ro splash";
const CMDLINE_SMALL: &str = "console=ttyS0 earlyprintk=ttyS0 quiet net.ifnames=0 biosdevname=0";
const CMDLINE_MEDIUM: &str = "console=ttyS0 earlyprintk=ttyS0 quiet ro net.ifnames=0 biosdevname=0 \
     server_url=http://10.0.0.1:9999 own_ip=10.0.0.42 tag=util-mdl:latest \
     hash=sha256:deadbeefcafe data_drive=/dev/sda2 base_url=registry.example.com/mtos";
const CMDLINE_VALUES_WITH_EQUALS: &str =
    "url=http://host/path?a=1&b=2 token=abc=def== other=x=y console=ttyS0";
// Boot parameter payloads for LUKS/LVM/NFS features.
// LUKS and LVM are now auto-detected from data_drive — no separate flags needed.
const CMDLINE_LUKS: &str =
    "console=ttyS0 quiet data_drive=/dev/sda2";
const CMDLINE_LVM: &str =
    "console=ttyS0 quiet data_drive=/dev/vg0/data";
const CMDLINE_LUKS_KEYFILE: &str =
    "console=ttyS0 quiet data_drive=/dev/sda2 luks_keyfile=/etc/luks.key";
const CMDLINE_NFS: &str = "console=ttyS0 quiet nfs_mount=192.168.1.1:/share:/mnt/nfs \
     nfs_opts=nolock,vers=4";
const CMDLINE_FULL: &str = "console=ttyS0 earlyprintk=ttyS0 quiet ro net.ifnames=0 biosdevname=0 \
     data_drive=/dev/sda2 luks_keyfile=/etc/luks.key \
     nfs_mount=192.168.1.1:/share:/mnt/nfs,10.0.0.2:/backup:/mnt/backup nfs_opts=nolock,vers=4 \
     server_url=http://10.0.0.1:9999 own_ip=10.0.0.42 tag=util-mdl:latest \
     hash=sha256:deadbeefcafe base_url=registry.example.com/mtos";

fn large_cmdline() -> String {
    (0..64)
        .map(|i| format!("key{i}=value{i}"))
        .collect::<Vec<_>>()
        .join(" ")
}

mod param_search {
    use super::*;

    #[test]
    fn empty() {
        black_box(CmdLineOptions::param_search(""));
    }

    #[test]
    fn bare_flags_only() {
        black_box(CmdLineOptions::param_search(CMDLINE_BARE_FLAGS));
    }

    #[test]
    fn small() {
        black_box(CmdLineOptions::param_search(CMDLINE_SMALL));
    }

    #[test]
    fn medium() {
        black_box(CmdLineOptions::param_search(CMDLINE_MEDIUM));
    }

    #[test]
    fn large_64_pairs() {
        let input = large_cmdline();
        black_box(CmdLineOptions::param_search(&input));
    }

    #[test]
    fn values_with_equals() {
        black_box(CmdLineOptions::param_search(CMDLINE_VALUES_WITH_EQUALS));
    }

    #[test]
    fn single_pair() {
        black_box(CmdLineOptions::param_search("console=ttyS0"));
    }

    #[test]
    fn luks_autodetect() {
        black_box(CmdLineOptions::param_search(CMDLINE_LUKS));
    }

    #[test]
    fn lvm_autodetect() {
        black_box(CmdLineOptions::param_search(CMDLINE_LVM));
    }

    #[test]
    fn luks_with_keyfile() {
        black_box(CmdLineOptions::param_search(CMDLINE_LUKS_KEYFILE));
    }

    #[test]
    fn nfs_params() {
        black_box(CmdLineOptions::param_search(CMDLINE_NFS));
    }

    #[test]
    fn full_boot_params() {
        black_box(CmdLineOptions::param_search(CMDLINE_FULL));
    }
}

mod param_search_scaling {
    use super::*;

    #[test]
    fn scaling() {
        for n in [1usize, 8, 16, 32, 64, 128] {
            let input: String = (0..n)
                .map(|i| format!("key{i}=value{i}"))
                .collect::<Vec<_>>()
                .join(" ");
            black_box(CmdLineOptions::param_search(input.as_str()));
        }
    }
}

mod reboot_cmd_dispatch {
    use super::*;

    #[test]
    fn init_bare() {
        black_box(RebootCMD::from("init"));
    }

    #[test]
    fn poweroff_bare() {
        black_box(RebootCMD::from("poweroff"));
    }

    #[test]
    fn reboot_bare() {
        black_box(RebootCMD::from("reboot"));
    }

    #[test]
    fn unknown_bare() {
        black_box(RebootCMD::from("shutdown"));
    }

    #[test]
    fn init_full_path() {
        black_box(RebootCMD::from("/bin/init"));
    }

    #[test]
    fn poweroff_full_path() {
        black_box(RebootCMD::from("/bin/poweroff"));
    }

    #[test]
    fn reboot_full_path() {
        black_box(RebootCMD::from("/bin/reboot"));
    }

    #[test]
    fn unknown_deep_path() {
        black_box(RebootCMD::from("/usr/local/sbin/some-unknown-tool"));
    }
}

mod reboot_cmd_conversions {
    use super::*;
    use rustix::system::RebootCommand;

    #[test]
    fn reboot_cmd_to_reboot_command() {
        let cmd = black_box(RebootCMD::Reboot);
        black_box(RebootCommand::from(cmd));
    }

    #[test]
    fn poweroff_cmd_to_reboot_command() {
        let cmd = black_box(RebootCMD::PowerOff);
        black_box(RebootCommand::from(cmd));
    }

    #[test]
    fn reboot_command_to_reboot_cmd() {
        black_box(RebootCMD::from(RebootCommand::Restart));
    }

    #[test]
    fn poweroff_command_to_reboot_cmd() {
        black_box(RebootCMD::from(RebootCommand::PowerOff));
    }
}

mod nfs_parsing {
    use super::*;

    #[test]
    fn single_mount() {
        black_box(parse_nfs_spec("192.168.1.1:/share:/mnt/nfs"));
    }

    #[test]
    fn two_mounts() {
        black_box(parse_nfs_spec(
            "192.168.1.1:/share:/mnt/nfs,10.0.0.2:/backup:/mnt/backup",
        ));
    }

    #[test]
    fn five_mounts() {
        black_box(parse_nfs_spec(
            "10.0.0.1:/a:/mnt/a,10.0.0.2:/b:/mnt/b,10.0.0.3:/c:/mnt/c,\
             10.0.0.4:/d:/mnt/d,10.0.0.5:/e:/mnt/e",
        ));
    }
}

mod preboot_construction {
    use super::*;

    #[test]
    fn new() {
        black_box(Preboot::new());
    }

    #[test]
    fn default() {
        black_box(Preboot::default());
    }

    #[test]
    fn clone() {
        let preboot = Preboot::new();
        black_box(preboot.clone());
    }
}
