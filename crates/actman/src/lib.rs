pub mod cmdline;
pub mod preboot;
pub mod reboot;

#[cfg(test)]
mod tests {
    // ── cmdline ───────────────────────────────────────────────────────────────
    use crate::cmdline::CmdLineOptions;

    #[test]
    fn parses_key_value_pairs() {
        let map = CmdLineOptions::param_search("console=ttyS0 earlyprintk=ttyS0".to_string());
        assert_eq!(map.get("console").map(String::as_str), Some("ttyS0"));
        assert_eq!(map.get("earlyprintk").map(String::as_str), Some("ttyS0"));
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn bare_flags_are_dropped() {
        let map = CmdLineOptions::param_search("quiet ro splash".to_string());
        assert!(map.is_empty(), "bare flags must be silently dropped");
    }

    #[test]
    fn mixed_flags_and_pairs() {
        let map = CmdLineOptions::param_search("quiet console=ttyS0 ro".to_string());
        assert_eq!(map.len(), 1);
        assert_eq!(map.get("console").map(String::as_str), Some("ttyS0"));
    }

    #[test]
    fn multiple_spaces_between_tokens() {
        let map = CmdLineOptions::param_search("  a=1   b=2  ".to_string());
        assert_eq!(map.get("a").map(String::as_str), Some("1"));
        assert_eq!(map.get("b").map(String::as_str), Some("2"));
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn empty_input_gives_empty_map() {
        let map = CmdLineOptions::param_search(String::new());
        assert!(map.is_empty());
    }

    #[test]
    fn value_containing_equals_splits_on_first_only() {
        let map = CmdLineOptions::param_search("url=http://host/path?a=1&b=2".to_string());
        assert_eq!(
            map.get("url").map(String::as_str),
            Some("http://host/path?a=1&b=2")
        );
        assert_eq!(map.len(), 1);
    }

    // ── preboot ───────────────────────────────────────────────────────────────
    use crate::preboot::{Preboot, VIRTUAL_FS};

    #[test]
    fn mounts_is_subset_of_virtual_fs() {
        let preboot = Preboot::default();
        for entry in &preboot.mounts {
            assert!(
                VIRTUAL_FS.contains(entry),
                "mounts contains {entry:?} which is not in VIRTUAL_FS"
            );
        }
    }

    #[test]
    fn mounts_entries_are_existing_directories() {
        let preboot = Preboot::default();
        for (name, _fstype) in &preboot.mounts {
            let path = std::path::Path::new("/").join(name);
            assert!(
                path.is_dir(),
                "/{name} should be a directory but is not present in this environment"
            );
        }
    }

    #[test]
    fn missing_directories_are_excluded() {
        let preboot = Preboot::default();
        for (name, _fstype) in VIRTUAL_FS {
            let exists = std::path::Path::new("/").join(name).is_dir();
            let in_mounts = preboot.mounts.iter().any(|(n, _)| n == name);
            assert_eq!(
                exists, in_mounts,
                "/{name}: exists={exists} but in_mounts={in_mounts} — filter is inconsistent"
            );
        }
    }

    #[test]
    fn new_and_default_are_equivalent() {
        let via_new = Preboot::new();
        let via_default = Preboot::default();
        assert_eq!(
            via_new.mounts, via_default.mounts,
            "Preboot::new() and Preboot::default() should produce the same mounts list"
        );
    }

    // ── reboot ────────────────────────────────────────────────────────────────
    use crate::reboot::RebootCMD;
    use rustix::system::RebootCommand;

    #[test]
    fn basename_init_maps_to_init() {
        assert_eq!(RebootCMD::from(&"init".to_string()), RebootCMD::Init);
    }

    #[test]
    fn basename_poweroff_maps_to_poweroff() {
        assert_eq!(
            RebootCMD::from(&"poweroff".to_string()),
            RebootCMD::PowerOff
        );
    }

    #[test]
    fn basename_reboot_maps_to_reboot() {
        assert_eq!(RebootCMD::from(&"reboot".to_string()), RebootCMD::Reboot);
    }

    #[test]
    fn unrecognised_basename_maps_to_cadoff() {
        assert_eq!(RebootCMD::from(&"foobar".to_string()), RebootCMD::CadOff);
        assert_eq!(RebootCMD::from(&"shutdown".to_string()), RebootCMD::CadOff);
    }

    #[test]
    fn path_prefixed_init_resolves_correctly() {
        assert_eq!(RebootCMD::from(&"/bin/init".to_string()), RebootCMD::Init);
    }

    #[test]
    fn path_prefixed_poweroff_resolves_correctly() {
        assert_eq!(
            RebootCMD::from(&"/bin/poweroff".to_string()),
            RebootCMD::PowerOff
        );
    }

    #[test]
    fn path_prefixed_reboot_resolves_correctly() {
        assert_eq!(
            RebootCMD::from(&"/bin/reboot".to_string()),
            RebootCMD::Reboot
        );
    }

    #[test]
    fn path_prefixed_unknown_maps_to_cadoff() {
        assert_eq!(
            RebootCMD::from(&"/usr/sbin/unknown".to_string()),
            RebootCMD::CadOff
        );
    }

    #[test]
    fn reboot_cmd_reboot_maps_to_restart() {
        let rc: RebootCommand = RebootCMD::Reboot.into();
        assert_eq!(rc, RebootCommand::Restart);
    }

    #[test]
    fn reboot_cmd_poweroff_maps_to_poweroff() {
        let rc: RebootCommand = RebootCMD::PowerOff.into();
        assert_eq!(rc, RebootCommand::PowerOff);
    }

    #[test]
    fn reboot_cmd_init_maps_to_cadoff() {
        let rc: RebootCommand = RebootCMD::Init.into();
        assert_eq!(rc, RebootCommand::CadOff);
    }

    #[test]
    fn reboot_cmd_cadoff_maps_to_cadoff() {
        let rc: RebootCommand = RebootCMD::CadOff.into();
        assert_eq!(rc, RebootCommand::CadOff);
    }

    #[test]
    fn reboot_command_restart_maps_to_reboot() {
        assert_eq!(RebootCMD::from(RebootCommand::Restart), RebootCMD::Reboot);
    }

    #[test]
    fn reboot_command_poweroff_maps_to_poweroff() {
        assert_eq!(
            RebootCMD::from(RebootCommand::PowerOff),
            RebootCMD::PowerOff
        );
    }

    #[test]
    fn reboot_command_cadoff_maps_to_cadoff() {
        assert_eq!(RebootCMD::from(RebootCommand::CadOff), RebootCMD::CadOff);
    }
}
