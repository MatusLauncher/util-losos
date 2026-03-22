use std::fs;
use std::path::{Path, PathBuf};

pub const LIMINE_REPO: &str = "https://github.com/limine-bootloader/limine.git";
pub const LIMINE_BRANCH: &str = "v10.x-binary";
pub const LIMINE_CONF: &str = r#"timeout: 5
default_entry: 1

/util-mdl
    protocol: linux
    path: boot():/boot/vmlinuz
    cmdline: quiet net.ifnames=0 biosdevname=0
    module_path: boot():/boot/initramfs.gz

/util-mdl (serial)
    protocol: linux
    path: boot():/boot/vmlinuz
    cmdline: console=ttyS0 earlyprintk=ttyS0 net.ifnames=0 biosdevname=0
    module_path: boot():/boot/initramfs.gz
"#;

pub fn resolve_output(base: &Path, raw: &str) -> PathBuf {
    let p = PathBuf::from(raw);
    if p.is_absolute() { p } else { base.join(p) }
}

pub fn scopeguard(path: &Path) -> impl Drop + use<'_> {
    struct Guard<'a>(&'a Path);
    impl Drop for Guard<'_> {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(self.0);
        }
    }
    Guard(path)
}

#[cfg(test)]
mod tests {
    use crate::{LIMINE_BRANCH, LIMINE_CONF, LIMINE_REPO, resolve_output, scopeguard};
    use std::path::PathBuf;
    use tempfile::TempDir;

    #[test]
    fn scopeguard_removes_directory_on_drop() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("staging");
        std::fs::create_dir_all(&path).unwrap();
        assert!(path.is_dir(), "directory should exist before drop");
        {
            let _guard = scopeguard(&path);
        }
        assert!(!path.exists(), "directory should be removed after drop");
    }

    #[test]
    fn scopeguard_is_noop_when_path_does_not_exist() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("nonexistent");
        assert!(!path.exists());
        {
            let _guard = scopeguard(&path);
        }
        assert!(!path.exists());
    }

    #[test]
    fn limine_conf_contains_default_entry() {
        assert!(LIMINE_CONF.contains("/util-mdl\n"));
    }

    #[test]
    fn limine_conf_contains_serial_entry() {
        assert!(LIMINE_CONF.contains("/util-mdl (serial)\n"));
    }

    #[test]
    fn limine_conf_has_two_entries() {
        let count = LIMINE_CONF
            .lines()
            .filter(|l: &&str| l.starts_with('/'))
            .count();
        assert_eq!(count, 2);
    }

    #[test]
    fn limine_conf_uses_linux_protocol_for_all_entries() {
        let count = LIMINE_CONF
            .lines()
            .filter(|l: &&str| l.trim() == "protocol: linux")
            .count();
        assert_eq!(count, 2);
    }

    #[test]
    fn limine_conf_references_kernel_path() {
        assert!(LIMINE_CONF.contains("path: boot():/boot/vmlinuz"));
    }

    #[test]
    fn limine_conf_references_initramfs_path() {
        assert!(LIMINE_CONF.contains("module_path: boot():/boot/initramfs.gz"));
    }

    #[test]
    fn limine_conf_serial_entry_has_console_cmdline() {
        let lines: Vec<&str> = LIMINE_CONF.lines().collect();
        let serial_pos = lines
            .iter()
            .position(|l| *l == "/util-mdl (serial)")
            .expect("serial entry must exist");
        let serial_section: String = lines[serial_pos..]
            .iter()
            .take_while(|l| !l.starts_with('/') || l.contains("serial"))
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        assert!(serial_section.contains("console=ttyS0"));
        assert!(serial_section.contains("earlyprintk=ttyS0"));
    }

    #[test]
    fn limine_conf_has_timeout_setting() {
        assert!(LIMINE_CONF.lines().any(|l: &str| l.starts_with("timeout:")));
    }

    #[test]
    fn limine_repo_is_github_https_url() {
        assert!(LIMINE_REPO.starts_with("https://github.com/"));
    }

    #[test]
    fn limine_branch_is_binary_release() {
        assert!(LIMINE_BRANCH.ends_with("-binary"));
    }

    #[test]
    fn absolute_output_path_is_kept_as_is() {
        let base = PathBuf::from("/some/cwd");
        assert_eq!(
            resolve_output(&base, "/tmp/my.iso"),
            PathBuf::from("/tmp/my.iso")
        );
    }

    #[test]
    fn relative_output_path_is_joined_to_base() {
        let base = PathBuf::from("/some/cwd");
        assert_eq!(
            resolve_output(&base, "my.iso"),
            PathBuf::from("/some/cwd/my.iso")
        );
    }

    #[test]
    fn relative_output_with_subdir_is_joined_to_base() {
        let base = PathBuf::from("/build");
        assert_eq!(
            resolve_output(&base, "out/images/disk.iso"),
            PathBuf::from("/build/out/images/disk.iso")
        );
    }
}
