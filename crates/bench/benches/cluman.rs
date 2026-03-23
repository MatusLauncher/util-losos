//! Benchmarks for the `cluman` crate.
//!
//! Covers:
//! * `IpRange::from_str`  — parsing all three notations.
//! * `IpRange::hosts`     — expanding single IPs, CIDR blocks, and dash ranges
//!                          at various sizes.
//! * `CluManSchema::add`  — map-insertion path from both the `Right` and `Left`
//!                          variants, plus a scaling sweep.
//! * `Tasks` push / pop   — throughput and single-operation latency for the
//!                          `VecDeque`-backed queue.
//! * `ServerState`        — lock-contended push/claim/read operations.
//! * `Mode` conversions   — `from_str` and `Display` for every variant.
//! * `Task` serialization — `serde_json` round-trips at small and large sizes.

use std::net::Ipv4Addr;
use std::str::FromStr;

use cluman::schemas::{CluManSchema, IpRange, Mode, ServerState, Task, Tasks};
use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};

// ── helpers ───────────────────────────────────────────────────────────────────

fn make_task(idx: usize) -> Task {
    Task::new(
        format!("compose-{idx:04}.yml"),
        format!("services:\n  svc{idx}:\n    image: nginx:{idx}\n"),
    )
}

fn make_task_large(idx: usize) -> Task {
    // ~4 KiB of YAML content to simulate a realistic compose file.
    let services: String = (0..64)
        .map(|s| {
            format!(
                "  svc{idx}_{s}:\n    image: registry.example.com/app{s}:v{idx}\n    \
                 environment:\n      - ENV_{s}=value_{idx}_{s}\n    ports:\n      - \"{p}:{p}\"\n",
                p = 8000 + s
            )
        })
        .collect();
    Task::new(format!("large-{idx:04}.yml"), services)
}

// ── IpRange::from_str ─────────────────────────────────────────────────────────

fn bench_ip_range_from_str(c: &mut Criterion) {
    let mut group = c.benchmark_group("ip_range/from_str");

    group.bench_function("single_ip", |b| {
        b.iter(|| IpRange::from_str("10.0.0.42").unwrap());
    });

    group.bench_function("cidr_slash24", |b| {
        b.iter(|| IpRange::from_str("192.168.1.0/24").unwrap());
    });

    group.bench_function("cidr_slash16", |b| {
        b.iter(|| IpRange::from_str("10.0.0.0/16").unwrap());
    });

    group.bench_function("dash_range_small", |b| {
        b.iter(|| IpRange::from_str("10.0.0.1-10.0.0.10").unwrap());
    });

    group.bench_function("dash_range_large", |b| {
        b.iter(|| IpRange::from_str("10.0.0.1-10.0.3.255").unwrap());
    });

    // Error paths — make sure rejection is equally cheap.
    group.bench_function("err_invalid_single", |b| {
        b.iter(|| IpRange::from_str("999.999.999.999").unwrap_err());
    });

    group.bench_function("err_reversed_dash_range", |b| {
        b.iter(|| IpRange::from_str("10.0.0.20-10.0.0.1").unwrap_err());
    });

    group.finish();
}

// ── IpRange::hosts ────────────────────────────────────────────────────────────

fn bench_ip_range_hosts(c: &mut Criterion) {
    let mut group = c.benchmark_group("ip_range/hosts");

    // Single — returns a one-element Vec, no iteration.
    group.bench_function("single", |b| {
        let r = IpRange::Single(Ipv4Addr::new(10, 0, 0, 1));
        b.iter(|| r.hosts());
    });

    // CIDR: tiny (2 hosts from /30).
    group.bench_function("cidr_slash30", |b| {
        let r = IpRange::from_str("192.168.0.0/30").unwrap();
        b.iter(|| r.hosts());
    });

    // CIDR: small (14 hosts from /28).
    group.bench_function("cidr_slash28", |b| {
        let r = IpRange::from_str("192.168.0.0/28").unwrap();
        b.iter(|| r.hosts());
    });

    // CIDR: medium (254 hosts from /24) — typical LAN segment.
    group.bench_function("cidr_slash24", |b| {
        let r = IpRange::from_str("10.0.1.0/24").unwrap();
        b.iter(|| r.hosts());
    });

    // CIDR: large (65 534 hosts from /16) — stresses Vec growth.
    group.bench_function("cidr_slash16", |b| {
        let r = IpRange::from_str("172.16.0.0/16").unwrap();
        b.iter(|| r.hosts());
    });

    // DashRange: tiny (5 addresses).
    group.bench_function("dash_range_5", |b| {
        let r = IpRange::DashRange(Ipv4Addr::new(10, 0, 0, 1), Ipv4Addr::new(10, 0, 0, 5));
        b.iter(|| r.hosts());
    });

    // DashRange: medium (256 addresses).
    group.bench_function("dash_range_256", |b| {
        let r = IpRange::DashRange(Ipv4Addr::new(10, 0, 0, 0), Ipv4Addr::new(10, 0, 0, 255));
        b.iter(|| r.hosts());
    });

    // DashRange: large (1 024 addresses).
    group.bench_function("dash_range_1024", |b| {
        let r = IpRange::DashRange(Ipv4Addr::new(10, 0, 0, 0), Ipv4Addr::new(10, 0, 3, 255));
        b.iter(|| r.hosts());
    });

    group.finish();
}

// ── IpRange::hosts — scaling sweep ───────────────────────────────────────────

fn bench_ip_range_hosts_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("ip_range/hosts_scaling");

    for n in [1u32, 10, 100, 256, 1_000, 10_000, 65_534] {
        let start = Ipv4Addr::from(0x0A000001u32); // 10.0.0.1
        let end = Ipv4Addr::from(0x0A000001u32 + n - 1);
        let r = IpRange::DashRange(start, end);
        group.bench_with_input(BenchmarkId::from_parameter(n), &r, |b, range| {
            b.iter(|| range.hosts());
        });
    }

    group.finish();
}

// ── CluManSchema::add ─────────────────────────────────────────────────────────

fn bench_cluman_schema_add(c: &mut Criterion) {
    let mut group = c.benchmark_group("cluman_schema/add");

    // First add: promotes `Right` → `Left` with one entry, then inserts.
    group.bench_function("first_add_from_right", |b| {
        b.iter_batched(
            || CluManSchema::registration(Ipv4Addr::new(10, 0, 0, 1), Mode::Client),
            |mut schema| {
                schema.add(Ipv4Addr::new(10, 0, 0, 2), Mode::Server);
                schema
            },
            BatchSize::SmallInput,
        );
    });

    // Subsequent add into an already-Left schema with a small map (10 entries).
    group.bench_function("add_to_small_map_10", |b| {
        b.iter_batched(
            || {
                let mut s = CluManSchema::default();
                for i in 0..10u8 {
                    s.add(Ipv4Addr::new(10, 0, 0, i), Mode::Client);
                }
                s
            },
            |mut schema| {
                schema.add(Ipv4Addr::new(10, 0, 1, 0), Mode::Client);
                schema
            },
            BatchSize::SmallInput,
        );
    });

    // Subsequent add into a medium map (100 entries).
    group.bench_function("add_to_medium_map_100", |b| {
        b.iter_batched(
            || {
                let mut s = CluManSchema::default();
                for i in 0u32..100 {
                    let ip = Ipv4Addr::from(0x0A000000u32 + i);
                    s.add(ip, Mode::Client);
                }
                s
            },
            |mut schema| {
                schema.add(Ipv4Addr::new(10, 1, 0, 0), Mode::Client);
                schema
            },
            BatchSize::SmallInput,
        );
    });

    // Subsequent add into a large map (1 000 entries).
    group.bench_function("add_to_large_map_1000", |b| {
        b.iter_batched(
            || {
                let mut s = CluManSchema::default();
                for i in 0u32..1_000 {
                    let ip = Ipv4Addr::from(0x0A000000u32 + i);
                    s.add(ip, Mode::Client);
                }
                s
            },
            |mut schema| {
                schema.add(Ipv4Addr::new(10, 5, 0, 0), Mode::Client);
                schema
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

// ── CluManSchema::add — scaling sweep ────────────────────────────────────────

fn bench_cluman_schema_add_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("cluman_schema/add_scaling");

    // Measure the total cost of building a registry from scratch.
    for n in [1usize, 10, 50, 100, 500] {
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &count| {
            b.iter(|| {
                let mut s = CluManSchema::default();
                for i in 0u32..count as u32 {
                    s.add(Ipv4Addr::from(0x0A000000u32 + i), Mode::Client);
                }
                s
            });
        });
    }

    group.finish();
}

// ── Tasks: individual operations ─────────────────────────────────────────────

fn bench_tasks_single_ops(c: &mut Criterion) {
    let mut group = c.benchmark_group("tasks/single_ops");

    // push_back on an empty queue — just allocates into a fresh VecDeque.
    group.bench_function("push_to_empty", |b| {
        b.iter_batched(
            Tasks::default,
            |mut q| {
                q.push(make_task(0));
                q
            },
            BatchSize::SmallInput,
        );
    });

    // push_back on a pre-filled queue of 1 000 tasks.
    group.bench_function("push_to_1000", |b| {
        b.iter_batched(
            || {
                let mut q = Tasks::default();
                for i in 0..1_000 {
                    q.push(make_task(i));
                }
                q
            },
            |mut q| {
                q.push(make_task(9999));
                q
            },
            BatchSize::SmallInput,
        );
    });

    // pop_front (O(1)) from a single-element queue.
    group.bench_function("pop_from_1", |b| {
        b.iter_batched(
            || {
                let mut q = Tasks::default();
                q.push(make_task(0));
                q
            },
            |mut q| q.pop(),
            BatchSize::SmallInput,
        );
    });

    // pop_front from a pre-filled queue of 1 000 tasks.
    group.bench_function("pop_from_1000", |b| {
        b.iter_batched(
            || {
                let mut q = Tasks::default();
                for i in 0..1_000 {
                    q.push(make_task(i));
                }
                q
            },
            |mut q| q.pop(),
            BatchSize::SmallInput,
        );
    });

    // is_empty on a non-empty queue.
    group.bench_function("is_empty_false", |b| {
        let mut q = Tasks::default();
        q.push(make_task(0));
        b.iter(|| q.is_empty());
    });

    // len read.
    group.bench_function("len_1000", |b| {
        let mut q = Tasks::default();
        for i in 0..1_000 {
            q.push(make_task(i));
        }
        b.iter(|| q.len());
    });

    group.finish();
}

// ── Tasks: throughput ─────────────────────────────────────────────────────────

fn bench_tasks_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("tasks/throughput");

    // Batch-push N tasks into an empty queue.
    for n in [10usize, 100, 1_000] {
        group.bench_with_input(BenchmarkId::new("push_n", n), &n, |b, &count| {
            b.iter_batched(
                Tasks::default,
                |mut q| {
                    for i in 0..count {
                        q.push(make_task(i));
                    }
                    q
                },
                BatchSize::SmallInput,
            );
        });
    }

    // Drain (pop all) from a pre-filled queue of N tasks.
    for n in [10usize, 100, 1_000] {
        group.bench_with_input(BenchmarkId::new("drain_n", n), &n, |b, &count| {
            b.iter_batched(
                || {
                    let mut q = Tasks::default();
                    for i in 0..count {
                        q.push(make_task(i));
                    }
                    q
                },
                |mut q| {
                    while q.pop().is_some() {}
                    q
                },
                BatchSize::SmallInput,
            );
        });
    }

    // Interleaved 1:1 push/pop — keeps the queue at depth ≈ 1.
    group.bench_function("interleaved_push_pop_1000", |b| {
        b.iter_batched(
            Tasks::default,
            |mut q| {
                for i in 0..1_000usize {
                    q.push(make_task(i));
                    let _ = q.pop();
                }
                q
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

// ── Tasks: construction from Vec ─────────────────────────────────────────────

fn bench_tasks_new(c: &mut Criterion) {
    let mut group = c.benchmark_group("tasks/new_from_vec");

    for n in [10usize, 100, 1_000] {
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &count| {
            b.iter_batched(
                || (0..count).map(make_task).collect::<Vec<_>>(),
                |v| Tasks::new(v),
                BatchSize::SmallInput,
            );
        });
    }

    group.finish();
}

// ── ServerState ───────────────────────────────────────────────────────────────

fn bench_server_state(c: &mut Criterion) {
    let mut group = c.benchmark_group("server_state");

    // push_task acquires the pending_tasks mutex, appends, then releases.
    group.bench_function("push_task", |b| {
        let s = ServerState::new();
        b.iter_batched(|| make_task(0), |t| s.push_task(t), BatchSize::SmallInput);
    });

    // claim_task acquires the same mutex and pops.
    group.bench_function("claim_task_from_nonempty", |b| {
        b.iter_batched(
            || {
                let s = ServerState::new();
                for i in 0..1_000 {
                    s.push_task(make_task(i));
                }
                s
            },
            |s| s.claim_task(),
            BatchSize::SmallInput,
        );
    });

    // claim_task on an empty queue — the common no-work case a client sees.
    group.bench_function("claim_task_from_empty", |b| {
        let s = ServerState::new();
        b.iter(|| s.claim_task());
    });

    // register_client acquires the clients mutex and inserts into a HashMap.
    group.bench_function("register_client", |b| {
        let s = ServerState::new();
        b.iter_batched(
            || Ipv4Addr::from(fastrand::u32(..)),
            |ip| s.register_client(ip),
            BatchSize::SmallInput,
        );
    });

    // pending_count — read-only lock acquisition.
    group.bench_function("pending_count", |b| {
        let s = ServerState::new();
        for i in 0..100 {
            s.push_task(make_task(i));
        }
        b.iter(|| s.pending_count());
    });

    // client_addrs — clones all keys out of the clients HashMap.
    group.bench_function("client_addrs_100", |b| {
        let s = ServerState::new();
        for i in 0u32..100 {
            s.register_client(Ipv4Addr::from(0x0A000000 + i));
        }
        b.iter(|| s.client_addrs());
    });

    // Full round-trip: push one task then immediately claim it.
    group.bench_function("push_then_claim_roundtrip", |b| {
        let s = ServerState::new();
        b.iter_batched(
            || make_task(0),
            |t| {
                s.push_task(t);
                s.claim_task()
            },
            BatchSize::SmallInput,
        );
    });

    group.finish();
}

// ── Mode conversions ──────────────────────────────────────────────────────────

fn bench_mode_conversions(c: &mut Criterion) {
    let mut group = c.benchmark_group("mode");

    group.bench_function("from_str_client", |b| {
        b.iter(|| Mode::from_str("client").unwrap());
    });

    group.bench_function("from_str_server", |b| {
        b.iter(|| Mode::from_str("server").unwrap());
    });

    group.bench_function("from_str_controller", |b| {
        b.iter(|| Mode::from_str("controller").unwrap());
    });

    group.bench_function("from_str_cluman_alias", |b| {
        b.iter(|| Mode::from_str("cluman").unwrap());
    });

    group.bench_function("from_str_err_unknown", |b| {
        b.iter(|| Mode::from_str("unknown").unwrap_err());
    });

    group.bench_function("display_client", |b| {
        b.iter(|| Mode::Client.to_string());
    });

    group.bench_function("display_server", |b| {
        b.iter(|| Mode::Server.to_string());
    });

    group.bench_function("display_controller", |b| {
        b.iter(|| Mode::Controller.to_string());
    });

    group.finish();
}

// ── Task serialisation ────────────────────────────────────────────────────────

fn bench_task_serde(c: &mut Criterion) {
    let mut group = c.benchmark_group("task/serde_json");

    // Small task (~100 B of JSON).
    group.bench_function("serialize_small", |b| {
        let t = make_task(0);
        b.iter(|| serde_json::to_string(&t).unwrap());
    });

    group.bench_function("deserialize_small", |b| {
        let json = serde_json::to_string(&make_task(0)).unwrap();
        b.iter(|| serde_json::from_str::<Task>(&json).unwrap());
    });

    // Large task (~4 KiB of YAML content).
    group.bench_function("serialize_large", |b| {
        let t = make_task_large(0);
        b.iter(|| serde_json::to_string(&t).unwrap());
    });

    group.bench_function("deserialize_large", |b| {
        let json = serde_json::to_string(&make_task_large(0)).unwrap();
        b.iter(|| serde_json::from_str::<Task>(&json).unwrap());
    });

    // Tasks queue round-trip (100 entries).
    group.bench_function("serialize_tasks_100", |b| {
        let q = Tasks::new((0..100).map(make_task).collect::<Vec<_>>());
        b.iter(|| serde_json::to_string(&q).unwrap());
    });

    group.bench_function("deserialize_tasks_100", |b| {
        let q = Tasks::new((0..100).map(make_task).collect::<Vec<_>>());
        let json = serde_json::to_string(&q).unwrap();
        b.iter(|| serde_json::from_str::<Tasks>(&json).unwrap());
    });

    group.finish();
}

// ── criterion wiring ──────────────────────────────────────────────────────────

criterion_group!(
    benches,
    bench_ip_range_from_str,
    bench_ip_range_hosts,
    bench_ip_range_hosts_scaling,
    bench_cluman_schema_add,
    bench_cluman_schema_add_scaling,
    bench_tasks_single_ops,
    bench_tasks_throughput,
    bench_tasks_new,
    bench_server_state,
    bench_mode_conversions,
    bench_task_serde,
);
criterion_main!(benches);
