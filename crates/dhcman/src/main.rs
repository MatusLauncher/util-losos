//! DHCP client with symlink polymorphism.
//!
//! The binary's `argv[0]` basename is the network interface to configure.
//! Symlinking `dhcman` to an interface name runs a full DHCP DORA sequence
//! on that interface and configures the address, route, and DNS.
//!
//! ```text
//! ln -sf /bin/dhcman /etc/init/start/eth0   # configures eth0 via DHCP
//! ln -sf /bin/dhcman /etc/init/start/wlan0  # configures wlan0 via DHCP
//! ```

use std::time::{Duration, Instant};

use miette::IntoDiagnostic;
use tracing::info;
use tracing_subscriber::fmt;

mod dhcp;
mod netconf;

/// Derive the network interface name from `argv[0]`.
///
/// 1. Takes the basename of the path (strips any directory prefix).
/// 2. If the basename starts with an all-digit segment followed by `-`
///    (e.g. `"01-eth0"`), the numeric ordering prefix is stripped and the
///    remainder (`"eth0"`) is returned.
/// 3. Otherwise the bare basename is returned as-is.
/// 4. Falls back to `"dhcman"` when the basename cannot be determined.
pub(crate) fn parse_iface(argv0: &str) -> &str {
    let base = std::path::Path::new(argv0)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("dhcman");
    // Strip optional numeric ordering prefix so that symlinking as
    // "01-eth0" configures the "eth0" interface.
    match base.split_once('-') {
        Some((prefix, rest)) if prefix.chars().all(|c| c.is_ascii_digit()) => rest,
        _ => base,
    }
}

fn main() -> miette::Result<()> {
    fmt().init();

    let args: Vec<_> = std::env::args().collect();
    let iface = parse_iface(&args[0]);

    if iface == "dhcman" {
        eprintln!("Usage: symlink dhcman to a network interface name to configure it via DHCP.");
        eprintln!("  Example: ln -sf /bin/dhcman /etc/init/start/eth0");
        return Ok(());
    }

    // Wait up to 10 s for the NIC to appear in sysfs (driver probe may lag boot).
    let sysfs = format!("/sys/class/net/{iface}");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !std::path::Path::new(&sysfs).exists() {
        if Instant::now() >= deadline {
            miette::bail!("interface {iface} did not appear in sysfs within 10 s");
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    info!("Bringing {iface} up");
    netconf::set_link_up(iface)?;

    info!("Running DHCP on {iface}");
    let lease = dhcp::acquire(iface)?;

    info!(
        "Setting {iface} address to {}/{}",
        lease.ip, lease.prefix_len
    );
    netconf::set_addr(iface, lease.ip, lease.prefix_len)?;

    if let Some(gw) = lease.gateway {
        info!("Adding default route via {gw}");
        netconf::add_default_route(gw)?;
    }

    if !lease.dns.is_empty() {
        let resolv = lease
            .dns
            .iter()
            .map(|ip| format!("nameserver {ip}\n"))
            .collect::<String>();
        std::fs::write("/etc/resolv.conf", resolv).into_diagnostic()?;
        info!("Wrote /etc/resolv.conf with {} server(s)", lease.dns.len());
    }

    info!("{iface} configured via DHCP");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::parse_iface;

    #[test]
    fn plain_interface_name_is_returned_unchanged() {
        assert_eq!(parse_iface("eth0"), "eth0");
    }

    #[test]
    fn numeric_prefix_is_stripped() {
        assert_eq!(parse_iface("01-eth0"), "eth0");
    }

    #[test]
    fn multi_digit_numeric_prefix_is_stripped() {
        assert_eq!(parse_iface("99-wlan0"), "wlan0");
    }

    #[test]
    fn deeper_path_with_numeric_prefix_is_stripped() {
        assert_eq!(parse_iface("/etc/init/start/01-eth0"), "eth0");
    }

    #[test]
    fn dhcman_basename_stays_as_dhcman() {
        assert_eq!(parse_iface("dhcman"), "dhcman");
    }

    #[test]
    fn full_path_to_dhcman_stays_as_dhcman() {
        assert_eq!(parse_iface("/bin/dhcman"), "dhcman");
    }

    #[test]
    fn non_numeric_prefix_is_not_stripped() {
        // "abc" is not all-digits, so the whole basename is kept.
        assert_eq!(parse_iface("abc-eth0"), "abc-eth0");
    }

    #[test]
    fn non_numeric_prefix_in_full_path_is_not_stripped() {
        assert_eq!(parse_iface("/etc/init/start/abc-eth0"), "abc-eth0");
    }

    #[test]
    fn interface_with_no_dash_in_full_path() {
        assert_eq!(parse_iface("/etc/init/start/enp3s0"), "enp3s0");
    }
}
