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

/// The IP configuration returned by a successful DHCP lease acquisition.
pub struct DhcpLease {
    /// The IPv4 address assigned by the DHCP server.
    pub ip: Ipv4Addr,
    /// Subnet prefix length derived from the offered subnet mask.
    pub prefix_len: u8,
    /// Default gateway, if provided in the DHCP offer.
    pub gateway: Option<Ipv4Addr>,
    /// DNS server addresses provided by the DHCP server.
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
///
/// The ID is seeded from the sub-second nanosecond component of the current
/// system time. If the system clock is before the Unix epoch (i.e.
/// [`SystemTime::duration_since`](std::time::SystemTime::duration_since)
/// returns an error), the value falls back to `0xdead_beef`.
pub fn new_xid() -> u32 {
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

/// Serialises a [`dhcproto::v4::Message`] into a byte vector.
pub fn encode(msg: &Message) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    msg.encode(&mut Encoder::new(&mut buf)).into_diagnostic()?;
    Ok(buf)
}

/// Deserialises a [`dhcproto::v4::Message`] from a byte slice.
pub fn decode(buf: &[u8]) -> Result<Message> {
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
///
/// The message has the broadcast flag set so that the server's reply reaches
/// the client before an IP address has been assigned. The parameter request
/// list asks for [`OptionCode::SubnetMask`], [`OptionCode::Router`], and
/// [`OptionCode::DomainNameServer`]. `xid` is the transaction ID (see
/// [`new_xid`]) and `mac` is the 6-byte hardware address of the interface.
pub fn discover(xid: u32, mac: &[u8; 6]) -> Message {
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
///
/// In addition to the standard boot-request fields, this message includes a
/// [`DhcpOption::RequestedIpAddress`] option set to `offered` (the IP
/// address proposed in the DHCPOFFER) and a [`DhcpOption::ServerIdentifier`]
/// option set to `server` (the IP of the offering DHCP server), as required
/// by RFC 2131 when transitioning from the SELECTING state.
pub fn request(xid: u32, mac: &[u8; 6], offered: Ipv4Addr, server: Ipv4Addr) -> Message {
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
///
/// Counts the number of leading one-bits in the 32-bit representation of
/// `mask`. For example: `mask_to_prefix(255.255.255.0) == 24`.
pub fn mask_to_prefix(mask: Ipv4Addr) -> u8 {
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

/// Perform the full DHCP DORA exchange on `iface`, retrying on failure.
///
/// Opens a single broadcast UDP socket via `dhcp_socket` and re-uses it
/// across all attempts. The handshake is attempted up to `MAX_ATTEMPTS` (3)
/// times; each attempt generates a fresh transaction ID. On the first
/// successful attempt the resulting [`DhcpLease`] is returned immediately.
/// If every attempt fails, the error from the final attempt is returned.
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
