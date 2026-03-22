pub mod dhcp;
pub mod netconf;

/// Derive the network interface name from `argv[0]`.
///
/// 1. Takes the basename of the path (strips any directory prefix).
/// 2. If the basename starts with an all-digit segment followed by `-`
///    (e.g. `"01-eth0"`), the numeric ordering prefix is stripped and the
///    remainder (`"eth0"`) is returned.
/// 3. Otherwise the bare basename is returned as-is.
/// 4. Falls back to `"dhcman"` when the basename cannot be determined.
pub fn parse_iface(argv0: &str) -> &str {
    let base = std::path::Path::new(argv0)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("dhcman");
    match base.split_once('-') {
        Some((prefix, rest)) if prefix.chars().all(|c| c.is_ascii_digit()) => rest,
        _ => base,
    }
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use dhcproto::v4::{DhcpOption, MessageType, OptionCode};

    use crate::dhcp::{decode, discover, encode, mask_to_prefix, new_xid, request};
    use crate::parse_iface;

    const DUMMY_MAC: [u8; 6] = [0xde, 0xad, 0xbe, 0xef, 0x00, 0x01];

    // ── parse_iface ───────────────────────────────────────────────────────────

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
    fn new_xid_does_not_panic() {
        // Smoke-test: new_xid() must never panic regardless of system clock
        // state.  Calling it many times exercises both the happy path and the
        // SystemTime-before-UNIX_EPOCH fallback branch.
        for _ in 0..64 {
            let _ = new_xid();
        }
    }

    #[test]
    fn new_xid_produces_varied_values() {
        // Generate a batch of XIDs.  Because new_xid() is seeded from
        // subsecond nanoseconds, a sufficiently large sample will almost
        // certainly contain at least two distinct values on any real clock.
        let xids: Vec<u32> = (0..64).map(|_| new_xid()).collect();
        let unique_count = {
            let mut seen = std::collections::HashSet::new();
            xids.iter().filter(|x| seen.insert(*x)).count()
        };
        assert!(
            unique_count > 1,
            "expected varied XID values across 64 calls, but all were identical ({:?})",
            xids[0]
        );
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
