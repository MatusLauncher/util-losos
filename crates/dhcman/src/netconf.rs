//! Network interface configuration via Linux netlink.
//!
//! Uses `libc` for raw socket operations (AF_NETLINK / NETLINK_ROUTE) and
//! `netlink-packet-*` for message building and parsing.
//! Everything is synchronous — no async runtime needed.

use std::net::{IpAddr, Ipv4Addr};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

use libc::{AF_NETLINK, NETLINK_ROUTE, SOCK_CLOEXEC, SOCK_RAW};
use miette::{Result, miette};
use netlink_packet_core::{
    Emitable, NLM_F_ACK, NLM_F_CREATE, NLM_F_EXCL, NLM_F_REQUEST, NetlinkHeader, NetlinkMessage,
    NetlinkPayload,
};
use netlink_packet_route::{
    AddressFamily, RouteNetlinkMessage,
    address::{AddressAttribute, AddressMessage},
    link::{LinkFlags, LinkMessage},
    route::{RouteAddress, RouteAttribute, RouteMessage, RouteProtocol, RouteScope, RouteType},
};
use tracing::debug;

/// Reads the kernel interface index for `iface` from `/sys/class/net/<iface>/ifindex`.
///
/// The file contains a single decimal integer written by the kernel; this
/// function trims whitespace and parses it into a `u32` suitable for use in
/// netlink message headers.
fn ifindex(iface: &str) -> Result<u32> {
    std::fs::read_to_string(format!("/sys/class/net/{iface}/ifindex"))
        .map_err(|e| miette!("cannot read ifindex for {iface}: {e}"))?
        .trim()
        .parse::<u32>()
        .map_err(|e| miette!("invalid ifindex for {iface}: {e}"))
}

/// Opens an `AF_NETLINK / NETLINK_ROUTE` raw socket with `SOCK_CLOEXEC`.
///
/// The `SOCK_CLOEXEC` flag is set atomically so the file descriptor is never
/// accidentally inherited across an `exec`. The returned [`OwnedFd`] closes
/// the socket automatically when dropped.
fn nl_socket() -> Result<OwnedFd> {
    let fd = unsafe { libc::socket(AF_NETLINK, SOCK_RAW | SOCK_CLOEXEC, NETLINK_ROUTE) };
    if fd < 0 {
        return Err(miette!(
            "netlink socket: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Finalises and serialises `msg` into a byte vector.
///
/// Calls [`NetlinkMessage::finalize`] to fill in the length and sequence-number
/// fields, then [`Emitable::emit`] to write the wire-format bytes into a
/// freshly allocated buffer.
fn encode<T>(mut msg: NetlinkMessage<T>) -> Vec<u8>
where
    T: netlink_packet_core::NetlinkSerializable,
    NetlinkMessage<T>: Emitable,
{
    msg.finalize();
    let mut buf = vec![0u8; msg.buffer_len()];
    msg.emit(&mut buf);
    buf
}

/// Sends `buf` to the kernel and reads the `NLMSG_ERROR` ack, returning an
/// error if the kernel reports a non-zero errno.
///
/// The datagram is addressed to pid 0 (the kernel). After the send, a single
/// reply datagram is received into a 4 KiB buffer and deserialised; if the
/// embedded error code is non-zero the absolute value is converted into a
/// human-readable [`std::io::Error`] and returned as a [`miette`] diagnostic.
fn nl_transact(sock: &OwnedFd, buf: &[u8]) -> Result<()> {
    // Send to kernel (pid = 0).
    let mut dst: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
    dst.nl_family = AF_NETLINK as libc::sa_family_t;

    let rc = unsafe {
        libc::sendto(
            sock.as_raw_fd(),
            buf.as_ptr().cast(),
            buf.len(),
            0,
            &dst as *const libc::sockaddr_nl as *const libc::sockaddr,
            std::mem::size_of::<libc::sockaddr_nl>() as libc::socklen_t,
        )
    };
    if rc < 0 {
        return Err(miette!(
            "netlink sendto: {}",
            std::io::Error::last_os_error()
        ));
    }

    // Read ack.
    let mut rbuf = vec![0u8; 4096];
    let n = unsafe { libc::recv(sock.as_raw_fd(), rbuf.as_mut_ptr().cast(), rbuf.len(), 0) };
    if n < 0 {
        return Err(miette!("netlink recv: {}", std::io::Error::last_os_error()));
    }

    let resp = NetlinkMessage::<RouteNetlinkMessage>::deserialize(&rbuf[..n as usize])
        .map_err(|e| miette!("netlink response parse: {e}"))?;

    if let NetlinkPayload::Error(e) = resp.payload
        && let Some(code) = e.code
    {
        return Err(miette!(
            "netlink error: {}",
            std::io::Error::from_raw_os_error(code.get().abs())
        ));
    }
    Ok(())
}

/// Bring the interface up (set `IFF_UP`).
///
/// The implementation:
/// 1. Reads the interface index from `/sys/class/net/<iface>/ifindex` via
///    `ifindex`.
/// 2. Opens an `AF_NETLINK / NETLINK_ROUTE` socket via `nl_socket`.
/// 3. Builds an `RTM_NEWLINK` message with the [`LinkFlags::Up`] flag set in
///    both `flags` and `change_mask`, then sends it to the kernel and waits
///    for the `NLMSG_ERROR` acknowledgement.
pub fn set_link_up(iface: &str) -> Result<()> {
    let idx = ifindex(iface)?;
    let sock = nl_socket()?;

    let mut link = LinkMessage::default();
    link.header.index = idx;
    link.header.flags = LinkFlags::Up;
    link.header.change_mask = LinkFlags::Up;

    let mut msg = NetlinkMessage::new(
        NetlinkHeader::default(),
        RouteNetlinkMessage::NewLink(link).into(),
    );
    msg.header.flags = NLM_F_REQUEST | NLM_F_ACK;

    nl_transact(&sock, &encode(msg))?;
    debug!("link {iface} is up");
    Ok(())
}

/// Assign `ip/prefix_len` to the interface.
///
/// Sends an `RTM_NEWADDRESS` message with flags `NLM_F_CREATE | NLM_F_EXCL`
/// so that the call succeeds only when the address does not already exist,
/// avoiding silent duplicates. Both the `Address` and `Local` attributes are
/// set to `ip` as required by the kernel for point-to-point and broadcast
/// interfaces alike.
pub fn set_addr(iface: &str, ip: Ipv4Addr, prefix_len: u8) -> Result<()> {
    let idx = ifindex(iface)?;
    let sock = nl_socket()?;

    let mut addr = AddressMessage::default();
    addr.header.family = AddressFamily::Inet;
    addr.header.prefix_len = prefix_len;
    addr.header.index = idx;
    addr.attributes
        .push(AddressAttribute::Address(IpAddr::V4(ip)));
    addr.attributes
        .push(AddressAttribute::Local(IpAddr::V4(ip)));

    let mut msg = NetlinkMessage::new(
        NetlinkHeader::default(),
        RouteNetlinkMessage::NewAddress(addr).into(),
    );
    msg.header.flags = NLM_F_REQUEST | NLM_F_ACK | NLM_F_CREATE | NLM_F_EXCL;

    nl_transact(&sock, &encode(msg))?;
    debug!("address {ip}/{prefix_len} set on {iface}");
    Ok(())
}

/// Install a default route (`0.0.0.0/0`) via `gateway`.
///
/// Sends an `RTM_NEWROUTE` message with:
/// - **protocol** `Boot` — indicates the route was installed by a boot-time
///   program (i.e. a DHCP client).
/// - **scope** `Universe` — the gateway is reachable via the broader network,
///   not link-local.
/// - **type** `Unicast` — a regular forwarding route.
/// - flags `NLM_F_CREATE | NLM_F_EXCL` — fails if a default route already
///   exists, preventing accidental overwrites.
pub fn add_default_route(gateway: Ipv4Addr) -> Result<()> {
    let sock = nl_socket()?;

    let mut route = RouteMessage::default();
    route.header.address_family = AddressFamily::Inet;
    route.header.destination_prefix_length = 0;
    route.header.protocol = RouteProtocol::Boot;
    route.header.scope = RouteScope::Universe;
    route.header.kind = RouteType::Unicast;
    route
        .attributes
        .push(RouteAttribute::Gateway(RouteAddress::Inet(gateway)));

    let mut msg = NetlinkMessage::new(
        NetlinkHeader::default(),
        RouteNetlinkMessage::NewRoute(route).into(),
    );
    msg.header.flags = NLM_F_REQUEST | NLM_F_ACK | NLM_F_CREATE | NLM_F_EXCL;

    nl_transact(&sock, &encode(msg))?;
    debug!("default route via {gateway} added");
    Ok(())
}
