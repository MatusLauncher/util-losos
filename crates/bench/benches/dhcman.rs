//! Smoke tests for the `dhcman` crate.
//!
//! Exercises:
//! * `parse_iface`        — interface-name extraction from `argv[0]`.
//! * `mask_to_prefix`     — subnet-mask → prefix-length conversion.
//! * `new_xid`            — transaction-ID generation.
//! * `discover`           — DHCPDISCOVER message construction.
//! * `request`            — DHCPREQUEST message construction.
//! * `encode`             — DHCP message serialisation to bytes.
//! * `decode`             — DHCP message deserialisation from bytes.
//! * Round-trips          — full encode→decode cycles.
//! * Throughput sweeps    — encode/decode at varying message counts.

use std::hint::black_box;
use std::net::Ipv4Addr;

use dhcman::{
    dhcp::{decode, discover, encode, mask_to_prefix, new_xid, request},
    parse_iface,
};

const DUMMY_MAC: [u8; 6] = [0xde, 0xad, 0xbe, 0xef, 0x00, 0x01];
const OFFERED_IP: Ipv4Addr = Ipv4Addr::new(192, 168, 1, 100);
const SERVER_IP: Ipv4Addr = Ipv4Addr::new(192, 168, 1, 1);
const FIXED_XID: u32 = 0x1234_5678;

fn discover_bytes() -> Vec<u8> {
    encode(&discover(FIXED_XID, &DUMMY_MAC)).expect("encode discover")
}

fn request_bytes() -> Vec<u8> {
    encode(&request(FIXED_XID, &DUMMY_MAC, OFFERED_IP, SERVER_IP)).expect("encode request")
}

mod parse_iface {
    use super::*;

    #[test]
    fn bare_eth0() {
        black_box(parse_iface("eth0"));
    }

    #[test]
    fn bare_wlan0() {
        black_box(parse_iface("wlan0"));
    }

    #[test]
    fn bare_enp3s0() {
        black_box(parse_iface("enp3s0"));
    }

    #[test]
    fn bare_dhcman() {
        black_box(parse_iface("dhcman"));
    }

    #[test]
    fn numeric_prefix_01_eth0() {
        black_box(parse_iface("01-eth0"));
    }

    #[test]
    fn numeric_prefix_99_wlan0() {
        black_box(parse_iface("99-wlan0"));
    }

    #[test]
    fn numeric_prefix_001_enp0s3() {
        black_box(parse_iface("001-enp0s3"));
    }

    #[test]
    fn non_numeric_prefix_abc_eth0() {
        black_box(parse_iface("abc-eth0"));
    }

    #[test]
    fn full_path_dhcman() {
        black_box(parse_iface("/bin/dhcman"));
    }

    #[test]
    fn full_path_bare_eth0() {
        black_box(parse_iface("/etc/init/start/eth0"));
    }

    #[test]
    fn full_path_prefixed_01_eth0() {
        black_box(parse_iface("/etc/init/start/01-eth0"));
    }

    #[test]
    fn full_path_non_numeric_prefix() {
        black_box(parse_iface("/etc/init/start/abc-eth0"));
    }

    #[test]
    fn deep_path_10_components() {
        black_box(parse_iface("/a/b/c/d/e/f/g/h/i/01-eth0"));
    }
}

mod mask_to_prefix {
    use super::*;

    #[test]
    fn all_standard_prefixes() {
        let masks: &[Ipv4Addr] = &[
            Ipv4Addr::new(0, 0, 0, 0),
            Ipv4Addr::new(128, 0, 0, 0),
            Ipv4Addr::new(255, 0, 0, 0),
            Ipv4Addr::new(255, 255, 0, 0),
            Ipv4Addr::new(255, 255, 255, 0),
            Ipv4Addr::new(255, 255, 255, 128),
            Ipv4Addr::new(255, 255, 255, 192),
            Ipv4Addr::new(255, 255, 255, 224),
            Ipv4Addr::new(255, 255, 255, 240),
            Ipv4Addr::new(255, 255, 255, 248),
            Ipv4Addr::new(255, 255, 255, 252),
            Ipv4Addr::new(255, 255, 255, 255),
        ];
        for &mask in masks {
            black_box(mask_to_prefix(mask));
        }
    }
}

mod new_xid {
    use super::*;

    #[test]
    fn single() {
        black_box(new_xid());
    }

    #[test]
    fn batch_16() {
        let mut xids = [0u32; 16];
        for x in xids.iter_mut() {
            *x = new_xid();
        }
        black_box(xids);
    }
}

mod dhcp_discover {
    use super::*;

    #[test]
    fn fixed_xid() {
        black_box(discover(FIXED_XID, &DUMMY_MAC));
    }

    #[test]
    fn live_xid() {
        black_box(discover(new_xid(), &DUMMY_MAC));
    }

    #[test]
    fn mac_variants() {
        let macs: &[[u8; 6]] = &[
            [0x00, 0x11, 0x22, 0x33, 0x44, 0x55],
            [0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff],
            [0xde, 0xad, 0xbe, 0xef, 0x00, 0x01],
            [0x52, 0x54, 0x00, 0x12, 0x34, 0x56],
        ];
        for mac in macs {
            black_box(discover(FIXED_XID, mac));
        }
    }
}

mod dhcp_request {
    use super::*;

    #[test]
    fn fixed_xid() {
        black_box(request(FIXED_XID, &DUMMY_MAC, OFFERED_IP, SERVER_IP));
    }

    #[test]
    fn live_xid() {
        black_box(request(new_xid(), &DUMMY_MAC, OFFERED_IP, SERVER_IP));
    }

    #[test]
    fn offered_ip_variants() {
        let offered_ips: &[Ipv4Addr] = &[
            Ipv4Addr::new(10, 0, 0, 1),
            Ipv4Addr::new(172, 16, 42, 100),
            Ipv4Addr::new(192, 168, 100, 200),
            Ipv4Addr::new(198, 18, 0, 1),
        ];
        for &offered in offered_ips {
            black_box(request(FIXED_XID, &DUMMY_MAC, offered, SERVER_IP));
        }
    }
}

mod dhcp_encode {
    use super::*;

    #[test]
    fn discover() {
        let msg = super::discover(FIXED_XID, &DUMMY_MAC);
        black_box(encode(&msg).unwrap());
    }

    #[test]
    fn request() {
        let msg = super::request(FIXED_XID, &DUMMY_MAC, OFFERED_IP, SERVER_IP);
        black_box(encode(&msg).unwrap());
    }

    #[test]
    fn discover_including_construction() {
        black_box(encode(&super::discover(new_xid(), &DUMMY_MAC)).unwrap());
    }

    #[test]
    fn request_including_construction() {
        black_box(encode(&super::request(new_xid(), &DUMMY_MAC, OFFERED_IP, SERVER_IP)).unwrap());
    }
}

mod dhcp_decode {
    use super::*;

    #[test]
    fn discover() {
        let bytes = discover_bytes();
        black_box(decode(&bytes).unwrap());
    }

    #[test]
    fn request() {
        let bytes = request_bytes();
        black_box(decode(&bytes).unwrap());
    }

    #[test]
    fn discover_fresh_bytes() {
        black_box(decode(&discover_bytes()).unwrap());
    }

    #[test]
    fn request_fresh_bytes() {
        black_box(decode(&request_bytes()).unwrap());
    }
}

mod encode_decode_roundtrip {
    use super::*;

    #[test]
    fn discover_construct_encode_decode() {
        let msg = super::discover(FIXED_XID, &DUMMY_MAC);
        let bytes = encode(&msg).unwrap();
        black_box(decode(&bytes).unwrap());
    }

    #[test]
    fn request_construct_encode_decode() {
        let msg = super::request(FIXED_XID, &DUMMY_MAC, OFFERED_IP, SERVER_IP);
        let bytes = encode(&msg).unwrap();
        black_box(decode(&bytes).unwrap());
    }

    #[test]
    fn discover_encode_decode_only() {
        let msg = super::discover(FIXED_XID, &DUMMY_MAC);
        let bytes = encode(&msg).unwrap();
        black_box(decode(&bytes).unwrap());
    }

    #[test]
    fn request_encode_decode_only() {
        let msg = super::request(FIXED_XID, &DUMMY_MAC, OFFERED_IP, SERVER_IP);
        let bytes = encode(&msg).unwrap();
        black_box(decode(&bytes).unwrap());
    }

    #[test]
    fn discover_live_xid_roundtrip() {
        let xid = new_xid();
        let msg = super::discover(xid, &DUMMY_MAC);
        let bytes = encode(&msg).unwrap();
        let decoded = decode(&bytes).unwrap();
        black_box(decoded.xid());
    }
}

mod encode_throughput {
    use super::*;

    #[test]
    fn discover_n() {
        for n in [1usize, 10, 100, 1_000] {
            let mut out = Vec::with_capacity(n);
            for i in 0..n as u32 {
                let msg = super::discover(FIXED_XID.wrapping_add(i), &DUMMY_MAC);
                out.push(encode(&msg).unwrap());
            }
            black_box(out);
        }
    }
}

mod decode_throughput {
    use super::*;

    #[test]
    fn discover_n() {
        let bytes = discover_bytes();
        for n in [1usize, 10, 100, 1_000] {
            let mut out = Vec::with_capacity(n);
            for _ in 0..n {
                out.push(decode(&bytes).unwrap());
            }
            black_box(out);
        }
    }
}

mod dora_encode_side {
    use super::*;

    #[test]
    fn discover_then_request() {
        let xid = new_xid();
        let disc_bytes = encode(&super::discover(xid, &DUMMY_MAC)).unwrap();
        black_box(decode(&disc_bytes).unwrap());
        let req_bytes = encode(&super::request(xid, &DUMMY_MAC, OFFERED_IP, SERVER_IP)).unwrap();
        black_box(decode(&req_bytes).unwrap());
    }

    #[test]
    fn repeated_dora_cycles() {
        for cycles in [1usize, 5, 10] {
            for _ in 0..cycles {
                let xid = new_xid();
                let disc = encode(&super::discover(xid, &DUMMY_MAC)).unwrap();
                black_box(decode(&disc).unwrap());
                let req = encode(&super::request(xid, &DUMMY_MAC, OFFERED_IP, SERVER_IP)).unwrap();
                black_box(decode(&req).unwrap());
            }
        }
    }
}
