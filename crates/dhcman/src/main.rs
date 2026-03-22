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

use dhcman::{dhcp, netconf, parse_iface};
use miette::IntoDiagnostic;
use tracing::info;
use tracing_subscriber::fmt;

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
