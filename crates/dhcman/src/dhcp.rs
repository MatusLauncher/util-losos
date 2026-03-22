//! DHCP DORA client (Discover → Offer → Request → Ack).
//!
//! Opens a broadcast UDP socket bound to the given interface, performs the
//! four-step handshake, and returns the acquired [`DhcpLease`].

use std::{
    net::{Ipv4Addr, UdpSocket},
    os::fd::FromRawFd,
    time::Duration,
};

use dhcproto::{
    Decodable, Decoder, Encodable, Encoder,
    v4::{DhcpOption, Flags, HType, Message, MessageType, Opcode, OptionCode},
};
use miette::{IntoDiagnostic, Result, bail};
use tracing::debug;

const SERVER_ADDR: &str = "255.255.255.255:67";
const CLIENT_PORT: u16 = 68;
const RECV_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_ATTEMPTS: usize = 3;

/// Result of a successful DHCP lease acquisition.
pub struct DhcpLease {
    pub ip: Ipv4Addr,
    pub prefix_len: u8,
    pub gateway: Option<Ipv4Addr>,
    pub dns: Vec<Ipv4Addr>,
}

/// Read the MAC address of `iface` from sysfs.
fn read_mac(iface: &str) -> Result<[u8; 6]> {
    let raw =
        std::fs::read_to_string(format!("/sys/class/net/{iface}/address")).into_diagnostic()?;
    let parts: Vec<u8> = raw
        .trim()
        .split(':')
        .map(|b| u8::from_str_radix(b, 16).into_diagnostic())
        .collect::<Result<_>>()?;
    if parts.len() != 6 {
        bail!("unexpected MAC format from sysfs: {raw:?}");
    }
    Ok([parts[0], parts[1], parts[2], parts[3], parts[4], parts[5]])
}

/// Derive a transaction ID from the current time.
pub(crate) fn new_xid() -> u32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0xdead_beef)
}

/// Open a broadcast UDP socket bound to `iface` on port 68.
/// SO_BINDTODEVICE must be set BEFORE bind() for the kernel to deliver
/// broadcast packets arriving on that interface before any IP is assigned.
fn dhcp_socket(iface: &str) -> Result<UdpSocket> {
    // Create socket manually so we can set options before bind().
    let fd = unsafe {
        libc::socket(
            libc::AF_INET,
            libc::SOCK_DGRAM | libc::SOCK_CLOEXEC,
            libc::IPPROTO_UDP,
        )
    };
    if fd < 0 {
        return Err(miette::miette!(
            "socket: {}",
            std::io::Error::last_os_error()
        ));
    }
    let one: libc::c_int = 1;
    unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_REUSEADDR,
            &one as *const _ as *const libc::c_void,
            std::mem::size_of_val(&one) as libc::socklen_t,
        );
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_BROADCAST,
            &one as *const _ as *const libc::c_void,
            std::mem::size_of_val(&one) as libc::socklen_t,
        );
    }
    // SO_BINDTODEVICE before bind().
    let iface_c = std::ffi::CString::new(iface).into_diagnostic()?;
    let rc = unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_BINDTODEVICE,
            iface_c.as_ptr().cast(),
            (iface.len() + 1) as libc::socklen_t,
        )
    };
    if rc != 0 {
        unsafe { libc::close(fd) };
        return Err(miette::miette!(
            "SO_BINDTODEVICE: {}",
            std::io::Error::last_os_error()
        ));
    }
    // Now bind to 0.0.0.0:68.
    let mut addr: libc::sockaddr_in = unsafe { std::mem::zeroed() };
    addr.sin_family = libc::AF_INET as libc::sa_family_t;
    addr.sin_port = CLIENT_PORT.to_be();
    addr.sin_addr.s_addr = 0;
    let rc = unsafe {
        libc::bind(
            fd,
            &addr as *const libc::sockaddr_in as *const libc::sockaddr,
            std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
        )
    };
    if rc != 0 {
        unsafe { libc::close(fd) };
        return Err(miette::miette!("bind: {}", std::io::Error::last_os_error()));
    }
    let udp = unsafe { UdpSocket::from_raw_fd(fd) };
    udp.set_read_timeout(Some(RECV_TIMEOUT)).into_diagnostic()?;
    Ok(udp)
}

pub(crate) fn encode(msg: &Message) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    msg.encode(&mut Encoder::new(&mut buf)).into_diagnostic()?;
    Ok(buf)
}

pub(crate) fn decode(buf: &[u8]) -> Result<Message> {
    Message::decode(&mut Decoder::new(buf)).into_diagnostic()
}

/// Receive packets until one with the expected `xid` arrives or we time out.
fn recv_reply(sock: &UdpSocket, buf: &mut [u8], xid: u32) -> Result<Message> {
    loop {
        match sock.recv(buf) {
            Ok(n) => match decode(&buf[..n]) {
                Ok(m) if m.xid() == xid => return Ok(m),
                _ => continue, // wrong xid or parse error
            },
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                bail!("DHCP receive timed out")
            }
            Err(e) => return Err(e).into_diagnostic(),
        }
    }
}

/// Build a DHCPDISCOVER message.
pub(crate) fn discover(xid: u32, mac: &[u8; 6]) -> Message {
    let mut msg = Message::default();
    msg.set_opcode(Opcode::BootRequest)
        .set_htype(HType::Unknown(1))
        .set_xid(xid)
        .set_flags(Flags::default().set_broadcast())
        .set_chaddr(mac); // also sets hlen
    msg.opts_mut()
        .insert(DhcpOption::MessageType(MessageType::Discover));
    msg.opts_mut().insert(DhcpOption::ParameterRequestList(vec![
        OptionCode::SubnetMask,
        OptionCode::Router,
        OptionCode::DomainNameServer,
    ]));
    msg
}

/// Build a DHCPREQUEST after receiving an offer.
pub(crate) fn request(xid: u32, mac: &[u8; 6], offered: Ipv4Addr, server: Ipv4Addr) -> Message {
    let mut msg = Message::default();
    msg.set_opcode(Opcode::BootRequest)
        .set_htype(HType::Unknown(1))
        .set_xid(xid)
        .set_flags(Flags::default().set_broadcast())
        .set_chaddr(mac);
    msg.opts_mut()
        .insert(DhcpOption::MessageType(MessageType::Request));
    msg.opts_mut()
        .insert(DhcpOption::RequestedIpAddress(offered));
    msg.opts_mut().insert(DhcpOption::ServerIdentifier(server));
    msg.opts_mut().insert(DhcpOption::ParameterRequestList(vec![
        OptionCode::SubnetMask,
        OptionCode::Router,
        OptionCode::DomainNameServer,
    ]));
    msg
}

/// Parse subnet mask → prefix length.
pub(crate) fn mask_to_prefix(mask: Ipv4Addr) -> u8 {
    u32::from(mask).leading_ones() as u8
}

/// Single DORA attempt. Returns the lease on success, or an error.
fn try_once(sock: &UdpSocket, mac: &[u8; 6], buf: &mut [u8]) -> Result<DhcpLease> {
    let xid = new_xid();

    // DISCOVER
    sock.send_to(&encode(&discover(xid, mac))?, SERVER_ADDR)
        .into_diagnostic()?;
    debug!("DHCPDISCOVER sent (xid={xid:#010x})");

    // OFFER
    let offer = recv_reply(sock, buf, xid)?;
    debug!("DHCPOFFER received: offered={}", offer.yiaddr());

    let server_id = match offer.opts().get(OptionCode::ServerIdentifier) {
        Some(DhcpOption::ServerIdentifier(ip)) => *ip,
        _ => bail!("DHCPOFFER missing server identifier"),
    };
    let offered_ip = offer.yiaddr();

    // REQUEST
    sock.send_to(
        &encode(&request(xid, mac, offered_ip, server_id))?,
        SERVER_ADDR,
    )
    .into_diagnostic()?;
    debug!("DHCPREQUEST sent");

    // ACK
    let ack = recv_reply(sock, buf, xid)?;
    match ack.opts().get(OptionCode::MessageType) {
        Some(DhcpOption::MessageType(MessageType::Ack)) => {}
        Some(DhcpOption::MessageType(MessageType::Nak)) => bail!("DHCPNAK received"),
        other => bail!("unexpected DHCP reply: {other:?}"),
    }

    let ip = ack.yiaddr();
    let prefix_len = match ack.opts().get(OptionCode::SubnetMask) {
        Some(DhcpOption::SubnetMask(mask)) => mask_to_prefix(*mask),
        _ => 24,
    };
    let gateway = match ack.opts().get(OptionCode::Router) {
        Some(DhcpOption::Router(routers)) => routers.first().copied(),
        _ => None,
    };
    let dns = match ack.opts().get(OptionCode::DomainNameServer) {
        Some(DhcpOption::DomainNameServer(servers)) => servers.clone(),
        _ => vec![],
    };

    debug!("DHCPACK: {ip}/{prefix_len} gw={gateway:?} dns={dns:?}");
    Ok(DhcpLease {
        ip,
        prefix_len,
        gateway,
        dns,
    })
}

/// Perform DHCP DORA, retrying up to `MAX_ATTEMPTS` times.
pub fn acquire(iface: &str) -> Result<DhcpLease> {
    let mac = read_mac(iface)?;
    let sock = dhcp_socket(iface)?;
    let mut buf = vec![0u8; 1500];

    let mut last_err = miette::miette!("no attempts made");
    for attempt in 1..=MAX_ATTEMPTS {
        match try_once(&sock, &mac, &mut buf) {
            Ok(lease) => return Ok(lease),
            Err(e) => {
                debug!("attempt {attempt}/{MAX_ATTEMPTS} failed: {e}");
                last_err = e;
            }
        }
    }
    Err(last_err)
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use dhcproto::v4::{DhcpOption, MessageType, OptionCode};

    use super::{decode, discover, encode, mask_to_prefix, new_xid, request};

    const DUMMY_MAC: [u8; 6] = [0xde, 0xad, 0xbe, 0xef, 0x00, 0x01];

    // ── mask_to_prefix ────────────────────────────────────────────────────────

    #[test]
    fn mask_to_prefix_slash0() {
        assert_eq!(mask_to_prefix(Ipv4Addr::new(0, 0, 0, 0)), 0);
    }

    #[test]
    fn mask_to_prefix_slash8() {
        assert_eq!(mask_to_prefix(Ipv4Addr::new(255, 0, 0, 0)), 8);
    }

    #[test]
    fn mask_to_prefix_slash16() {
        assert_eq!(mask_to_prefix(Ipv4Addr::new(255, 255, 0, 0)), 16);
    }

    #[test]
    fn mask_to_prefix_slash24() {
        assert_eq!(mask_to_prefix(Ipv4Addr::new(255, 255, 255, 0)), 24);
    }

    #[test]
    fn mask_to_prefix_slash32() {
        assert_eq!(mask_to_prefix(Ipv4Addr::new(255, 255, 255, 255)), 32);
    }

    // ── new_xid ───────────────────────────────────────────────────────────────

    #[test]
    fn new_xid_is_in_u32_range() {
        // u32 is always >= 0 by definition; call twice to confirm no panic.
        let x1 = new_xid();
        let x2 = new_xid();
        // Both values are valid u32s (the type system guarantees this, but we
        // also assert the range explicitly to make the intent clear).
        assert!(x1 <= u32::MAX);
        assert!(x2 <= u32::MAX);
    }

    // ── encode + decode round-trip on DISCOVER ────────────────────────────────

    #[test]
    fn discover_round_trip_preserves_xid() {
        let xid = 0x1234_5678u32;
        let msg = discover(xid, &DUMMY_MAC);
        let bytes = encode(&msg).expect("encode failed");
        let restored = decode(&bytes).expect("decode failed");
        assert_eq!(restored.xid(), xid);
    }

    #[test]
    fn discover_round_trip_preserves_message_type() {
        let xid = 0xdead_beefu32;
        let msg = discover(xid, &DUMMY_MAC);
        let bytes = encode(&msg).expect("encode failed");
        let restored = decode(&bytes).expect("decode failed");
        match restored.opts().get(OptionCode::MessageType) {
            Some(DhcpOption::MessageType(MessageType::Discover)) => {}
            other => panic!("expected Discover, got {other:?}"),
        }
    }

    // ── encode + decode round-trip on REQUEST ─────────────────────────────────

    #[test]
    fn request_round_trip_preserves_xid() {
        let xid = 0xabcd_ef01u32;
        let offered = Ipv4Addr::new(192, 168, 1, 100);
        let server = Ipv4Addr::new(192, 168, 1, 1);
        let msg = request(xid, &DUMMY_MAC, offered, server);
        let bytes = encode(&msg).expect("encode failed");
        let restored = decode(&bytes).expect("decode failed");
        assert_eq!(restored.xid(), xid);
    }

    #[test]
    fn request_round_trip_preserves_offered_ip() {
        let xid = 0x0000_0001u32;
        let offered = Ipv4Addr::new(10, 0, 0, 42);
        let server = Ipv4Addr::new(10, 0, 0, 1);
        let msg = request(xid, &DUMMY_MAC, offered, server);
        let bytes = encode(&msg).expect("encode failed");
        let restored = decode(&bytes).expect("decode failed");
        match restored.opts().get(OptionCode::RequestedIpAddress) {
            Some(DhcpOption::RequestedIpAddress(ip)) => assert_eq!(*ip, offered),
            other => panic!("expected RequestedIpAddress, got {other:?}"),
        }
    }

    #[test]
    fn request_round_trip_preserves_message_type() {
        let xid = 0x0000_0002u32;
        let offered = Ipv4Addr::new(172, 16, 0, 5);
        let server = Ipv4Addr::new(172, 16, 0, 1);
        let msg = request(xid, &DUMMY_MAC, offered, server);
        let bytes = encode(&msg).expect("encode failed");
        let restored = decode(&bytes).expect("decode failed");
        match restored.opts().get(OptionCode::MessageType) {
            Some(DhcpOption::MessageType(MessageType::Request)) => {}
            other => panic!("expected Request, got {other:?}"),
        }
    }
}
