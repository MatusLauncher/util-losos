//! Benchmarks for the `dhcman` crate.
//!
//! Covers:
//! * `parse_iface`        — interface-name extraction from `argv[0]`, all
//!                          naming variants (bare, numeric-prefixed, full path).
//! * `mask_to_prefix`     — subnet-mask → prefix-length conversion for every
//!                          standard mask.
//! * `new_xid`            — transaction-ID generation from the system clock.
//! * `discover`           — DHCPDISCOVER message construction.
//! * `request`            — DHCPREQUEST message construction.
//! * `encode`             — DHCP message serialisation to bytes.
//! * `decode`             — DHCP message deserialisation from bytes.
//! * Round-trips          — full encode→decode cycles for both message types.
//! * Throughput sweeps    — encode/decode at varying message counts.

use std::net::Ipv4Addr;

use criterion::{criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion};
use dhcman::{
    dhcp::{decode, discover, encode, mask_to_prefix, new_xid, request},
    parse_iface,
};

// ── shared fixtures ───────────────────────────────────────────────────────────

const DUMMY_MAC: [u8; 6] = [0xde, 0xad, 0xbe, 0xef, 0x00, 0x01];
const OFFERED_IP: Ipv4Addr = Ipv4Addr::new(192, 168, 1, 100);
const SERVER_IP: Ipv4Addr = Ipv4Addr::new(192, 168, 1, 1);
const FIXED_XID: u32 = 0x1234_5678;

/// Pre-encoded DHCPDISCOVER bytes (built once, reused in decode benchmarks).
fn discover_bytes() -> Vec<u8> {
    encode(&discover(FIXED_XID, &DUMMY_MAC)).expect("encode discover")
}

/// Pre-encoded DHCPREQUEST bytes (built once, reused in decode benchmarks).
fn request_bytes() -> Vec<u8> {
    encode(&request(FIXED_XID, &DUMMY_MAC, OFFERED_IP, SERVER_IP)).expect("encode request")
}

// ── parse_iface ───────────────────────────────────────────────────────────────

fn bench_parse_iface(c: &mut Criterion) {
    let mut group = c.benchmark_group("parse_iface");

    // Bare interface names — no path component, no numeric prefix.
    group.bench_function("bare_eth0", |b| {
        b.iter(|| parse_iface("eth0"));
    });

    group.bench_function("bare_wlan0", |b| {
        b.iter(|| parse_iface("wlan0"));
    });

    group.bench_function("bare_enp3s0", |b| {
        b.iter(|| parse_iface("enp3s0"));
    });

    group.bench_function("bare_dhcman", |b| {
        b.iter(|| parse_iface("dhcman"));
    });

    // Numeric-prefixed names — exercises the split_once('-') + all-digits check.
    group.bench_function("numeric_prefix_01_eth0", |b| {
        b.iter(|| parse_iface("01-eth0"));
    });

    group.bench_function("numeric_prefix_99_wlan0", |b| {
        b.iter(|| parse_iface("99-wlan0"));
    });

    group.bench_function("numeric_prefix_001_enp0s3", |b| {
        b.iter(|| parse_iface("001-enp0s3"));
    });

    // Non-numeric prefix — must NOT strip the prefix.
    group.bench_function("non_numeric_prefix_abc_eth0", |b| {
        b.iter(|| parse_iface("abc-eth0"));
    });

    // Full absolute paths — exercises the Path::file_name() strip.
    group.bench_function("full_path_dhcman", |b| {
        b.iter(|| parse_iface("/bin/dhcman"));
    });

    group.bench_function("full_path_bare_eth0", |b| {
        b.iter(|| parse_iface("/etc/init/start/eth0"));
    });

    group.bench_function("full_path_prefixed_01_eth0", |b| {
        b.iter(|| parse_iface("/etc/init/start/01-eth0"));
    });

    group.bench_function("full_path_non_numeric_prefix", |b| {
        b.iter(|| parse_iface("/etc/init/start/abc-eth0"));
    });

    // Deep path — many directory components to strip.
    group.bench_function("deep_path_10_components", |b| {
        b.iter(|| parse_iface("/a/b/c/d/e/f/g/h/i/01-eth0"));
    });

    group.finish();
}

// ── mask_to_prefix ────────────────────────────────────────────────────────────

fn bench_mask_to_prefix(c: &mut Criterion) {
    let mut group = c.benchmark_group("mask_to_prefix");

    // All standard prefix lengths from /0 to /32.
    let masks: &[(u8, Ipv4Addr)] = &[
        (0, Ipv4Addr::new(0, 0, 0, 0)),
        (1, Ipv4Addr::new(128, 0, 0, 0)),
        (8, Ipv4Addr::new(255, 0, 0, 0)),
        (16, Ipv4Addr::new(255, 255, 0, 0)),
        (24, Ipv4Addr::new(255, 255, 255, 0)),
        (25, Ipv4Addr::new(255, 255, 255, 128)),
        (26, Ipv4Addr::new(255, 255, 255, 192)),
        (27, Ipv4Addr::new(255, 255, 255, 224)),
        (28, Ipv4Addr::new(255, 255, 255, 240)),
        (29, Ipv4Addr::new(255, 255, 255, 248)),
        (30, Ipv4Addr::new(255, 255, 255, 252)),
        (32, Ipv4Addr::new(255, 255, 255, 255)),
    ];

    for (prefix, mask) in masks {
        group.bench_with_input(BenchmarkId::new("prefix", prefix), mask, |b, &m| {
            b.iter(|| mask_to_prefix(m));
        });
    }

    group.finish();
}

// ── new_xid ───────────────────────────────────────────────────────────────────

fn bench_new_xid(c: &mut Criterion) {
    let mut group = c.benchmark_group("new_xid");

    // Single call — measures the SystemTime::now() + duration_since overhead.
    group.bench_function("single", |b| {
        b.iter(new_xid);
    });

    // Generate a batch of 16 XIDs — representative of opening multiple parallel
    // DHCP sessions (e.g. configuring several virtual NICs at boot).
    group.bench_function("batch_16", |b| {
        b.iter(|| {
            let mut xids = [0u32; 16];
            for x in xids.iter_mut() {
                *x = new_xid();
            }
            xids
        });
    });

    group.finish();
}

// ── discover message construction ─────────────────────────────────────────────

fn bench_discover(c: &mut Criterion) {
    let mut group = c.benchmark_group("dhcp_message/discover");

    // Fixed XID — isolates message construction from clock overhead.
    group.bench_function("fixed_xid", |b| {
        b.iter(|| discover(FIXED_XID, &DUMMY_MAC));
    });

    // Live XID — as it runs in production.
    group.bench_function("live_xid", |b| {
        b.iter(|| discover(new_xid(), &DUMMY_MAC));
    });

    // Different MAC addresses — confirms the benchmark is not trivially cached.
    let macs: &[[u8; 6]] = &[
        [0x00, 0x11, 0x22, 0x33, 0x44, 0x55],
        [0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff],
        [0xde, 0xad, 0xbe, 0xef, 0x00, 0x01],
        [0x52, 0x54, 0x00, 0x12, 0x34, 0x56], // QEMU default MAC
    ];
    for (i, mac) in macs.iter().enumerate() {
        group.bench_with_input(BenchmarkId::new("mac_variant", i), mac, |b, m| {
            b.iter(|| discover(FIXED_XID, m));
        });
    }

    group.finish();
}

// ── request message construction ─────────────────────────────────────────────

fn bench_request(c: &mut Criterion) {
    let mut group = c.benchmark_group("dhcp_message/request");

    // Fixed XID — isolates message construction.
    group.bench_function("fixed_xid", |b| {
        b.iter(|| request(FIXED_XID, &DUMMY_MAC, OFFERED_IP, SERVER_IP));
    });

    // Live XID — production path.
    group.bench_function("live_xid", |b| {
        b.iter(|| request(new_xid(), &DUMMY_MAC, OFFERED_IP, SERVER_IP));
    });

    // Varying offered IPs — representative of different address ranges.
    let offered_ips: &[Ipv4Addr] = &[
        Ipv4Addr::new(10, 0, 0, 1),
        Ipv4Addr::new(172, 16, 42, 100),
        Ipv4Addr::new(192, 168, 100, 200),
        Ipv4Addr::new(198, 18, 0, 1),
    ];
    for (i, ip) in offered_ips.iter().enumerate() {
        group.bench_with_input(
            BenchmarkId::new("offered_ip_variant", i),
            ip,
            |b, &offered| {
                b.iter(|| request(FIXED_XID, &DUMMY_MAC, offered, SERVER_IP));
            },
        );
    }

    group.finish();
}

// ── encode ────────────────────────────────────────────────────────────────────

fn bench_encode(c: &mut Criterion) {
    let mut group = c.benchmark_group("dhcp_message/encode");

    // Encode a DHCPDISCOVER.
    group.bench_function("discover", |b| {
        let msg = discover(FIXED_XID, &DUMMY_MAC);
        b.iter(|| encode(&msg).unwrap());
    });

    // Encode a DHCPREQUEST (carries more options — slightly larger).
    group.bench_function("request", |b| {
        let msg = request(FIXED_XID, &DUMMY_MAC, OFFERED_IP, SERVER_IP);
        b.iter(|| encode(&msg).unwrap());
    });

    // Encode with a fresh message every iteration — includes construction cost.
    group.bench_function("discover_including_construction", |b| {
        b.iter(|| encode(&discover(new_xid(), &DUMMY_MAC)).unwrap());
    });

    group.bench_function("request_including_construction", |b| {
        b.iter(|| encode(&request(new_xid(), &DUMMY_MAC, OFFERED_IP, SERVER_IP)).unwrap());
    });

    group.finish();
}

// ── decode ────────────────────────────────────────────────────────────────────

fn bench_decode(c: &mut Criterion) {
    let mut group = c.benchmark_group("dhcp_message/decode");

    // Decode a DHCPDISCOVER packet.
    group.bench_function("discover", |b| {
        let bytes = discover_bytes();
        b.iter(|| decode(&bytes).unwrap());
    });

    // Decode a DHCPREQUEST packet.
    group.bench_function("request", |b| {
        let bytes = request_bytes();
        b.iter(|| decode(&bytes).unwrap());
    });

    // Decode with bytes allocated fresh each iteration — measures the pure
    // parsing cost when the input slice is hot in cache.
    group.bench_function("discover_fresh_bytes_each_iter", |b| {
        b.iter_batched(
            discover_bytes,
            |bytes| decode(&bytes).unwrap(),
            BatchSize::SmallInput,
        );
    });

    group.bench_function("request_fresh_bytes_each_iter", |b| {
        b.iter_batched(
            request_bytes,
            |bytes| decode(&bytes).unwrap(),
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

// ── encode → decode round-trip ────────────────────────────────────────────────

fn bench_encode_decode_roundtrip(c: &mut Criterion) {
    let mut group = c.benchmark_group("dhcp_message/roundtrip");

    // Full DISCOVER round-trip: construct → encode → decode.
    group.bench_function("discover_construct_encode_decode", |b| {
        b.iter(|| {
            let msg = discover(FIXED_XID, &DUMMY_MAC);
            let bytes = encode(&msg).unwrap();
            decode(&bytes).unwrap()
        });
    });

    // Full REQUEST round-trip.
    group.bench_function("request_construct_encode_decode", |b| {
        b.iter(|| {
            let msg = request(FIXED_XID, &DUMMY_MAC, OFFERED_IP, SERVER_IP);
            let bytes = encode(&msg).unwrap();
            decode(&bytes).unwrap()
        });
    });

    // Encode-only round-trip (pre-constructed message): encode → decode.
    group.bench_function("discover_encode_decode_only", |b| {
        let msg = discover(FIXED_XID, &DUMMY_MAC);
        b.iter(|| {
            let bytes = encode(&msg).unwrap();
            decode(&bytes).unwrap()
        });
    });

    group.bench_function("request_encode_decode_only", |b| {
        let msg = request(FIXED_XID, &DUMMY_MAC, OFFERED_IP, SERVER_IP);
        b.iter(|| {
            let bytes = encode(&msg).unwrap();
            decode(&bytes).unwrap()
        });
    });

    // Live-XID round-trip — as close to production as possible without a
    // real network socket.
    group.bench_function("discover_live_xid_roundtrip", |b| {
        b.iter(|| {
            let xid = new_xid();
            let msg = discover(xid, &DUMMY_MAC);
            let bytes = encode(&msg).unwrap();
            let decoded = decode(&bytes).unwrap();
            decoded.xid()
        });
    });

    group.finish();
}

// ── throughput scaling ────────────────────────────────────────────────────────

fn bench_encode_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("dhcp_message/encode_throughput");

    // How long does it take to encode N DISCOVER messages?
    for n in [1usize, 10, 100, 1_000] {
        group.bench_with_input(BenchmarkId::new("discover_n", n), &n, |b, &count| {
            b.iter(|| {
                let mut out = Vec::with_capacity(count);
                for i in 0..count as u32 {
                    let msg = discover(FIXED_XID.wrapping_add(i), &DUMMY_MAC);
                    out.push(encode(&msg).unwrap());
                }
                out
            });
        });
    }

    group.finish();
}

fn bench_decode_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("dhcp_message/decode_throughput");

    // How long does it take to decode N identical DISCOVER packets?
    // Models a server receiving a burst of duplicate DHCP broadcasts.
    for n in [1usize, 10, 100, 1_000] {
        group.bench_with_input(BenchmarkId::new("discover_n", n), &n, |b, &count| {
            let bytes = discover_bytes();
            b.iter(|| {
                let mut out = Vec::with_capacity(count);
                for _ in 0..count {
                    out.push(decode(&bytes).unwrap());
                }
                out
            });
        });
    }

    group.finish();
}

// ── DORA simulation ───────────────────────────────────────────────────────────

/// Simulates the encoding side of a full DORA exchange (Discover + Request)
/// without any network I/O — useful for measuring pure protocol overhead.
fn bench_dora_encode_side(c: &mut Criterion) {
    let mut group = c.benchmark_group("dhcp_message/dora_encode_side");

    group.bench_function("discover_then_request", |b| {
        b.iter(|| {
            let xid = new_xid();

            // Step 1: encode DISCOVER
            let disc_bytes = encode(&discover(xid, &DUMMY_MAC)).unwrap();

            // Step 2: decode the OFFER (simulated as re-decoding our DISCOVER
            // to isolate serialisation cost from network latency).
            let _ = decode(&disc_bytes).unwrap();

            // Step 3: encode REQUEST
            let req_bytes = encode(&request(xid, &DUMMY_MAC, OFFERED_IP, SERVER_IP)).unwrap();

            // Step 4: decode the ACK (simulated).
            let _ = decode(&req_bytes).unwrap();
        });
    });

    // Repeated DORA cycles — models a node that requests a renewal or retries.
    for n in [1usize, 5, 10] {
        group.bench_with_input(
            BenchmarkId::new("repeated_dora_cycles", n),
            &n,
            |b, &cycles| {
                b.iter(|| {
                    for _ in 0..cycles {
                        let xid = new_xid();
                        let disc = encode(&discover(xid, &DUMMY_MAC)).unwrap();
                        let _ = decode(&disc).unwrap();
                        let req = encode(&request(xid, &DUMMY_MAC, OFFERED_IP, SERVER_IP)).unwrap();
                        let _ = decode(&req).unwrap();
                    }
                });
            },
        );
    }

    group.finish();
}

// ── criterion wiring ──────────────────────────────────────────────────────────

criterion_group!(
    benches,
    bench_parse_iface,
    bench_mask_to_prefix,
    bench_new_xid,
    bench_discover,
    bench_request,
    bench_encode,
    bench_decode,
    bench_encode_decode_roundtrip,
    bench_encode_throughput,
    bench_decode_throughput,
    bench_dora_encode_side,
);
criterion_main!(benches);
