//! Smoke tokio::tests for the `cluman` crate.
//!
//! Exercises:
//! * `IpRange::from_str`  — parsing all three notations.
//! * `IpRange::hosts`     — expanding single IPs, CIDR blocks, and dash ranges.
//! * `CluManSchema::add`  — map-insertion from both the `Right` and `Left` variants.
//! * `Tasks` push / pop   — throughput and single-operation paths.
//! * `ServerState`        — push/claim/read operations.
//! * `Mode` conversions   — `from_str` and `Display` for every variant.
//! * `Task` serialization — `serde_json` round-trips at small and large sizes.

use std::hint::black_box;
use std::net::Ipv4Addr;
use std::str::FromStr;

use cluman::schemas::{CluManSchema, IpRange, Mode, ServerState, Task, Tasks};

async fn make_task(idx: usize) -> Task {
    Task::new(
        format!("compose-{idx:04}.yml"),
        format!("services:\n  svc{idx}:\n    image: nginx:{idx}\n"),
    )
}

async fn make_task_large(idx: usize) -> Task {
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

mod ip_range_from_str {
    use super::*;

    #[tokio::test]
    async fn single_ip() {
        black_box(IpRange::from_str("10.0.0.42").unwrap());
    }

    #[tokio::test]
    async fn cidr_slash24() {
        black_box(IpRange::from_str("192.168.1.0/24").unwrap());
    }

    #[tokio::test]
    async fn cidr_slash16() {
        black_box(IpRange::from_str("10.0.0.0/16").unwrap());
    }

    #[tokio::test]
    async fn dash_range_small() {
        black_box(IpRange::from_str("10.0.0.1-10.0.0.10").unwrap());
    }

    #[tokio::test]
    async fn dash_range_large() {
        black_box(IpRange::from_str("10.0.0.1-10.0.3.255").unwrap());
    }

    #[tokio::test]
    async fn err_invalid_single() {
        black_box(IpRange::from_str("999.999.999.999").unwrap_err());
    }

    #[tokio::test]
    async fn err_reversed_dash_range() {
        black_box(IpRange::from_str("10.0.0.20-10.0.0.1").unwrap_err());
    }
}

mod ip_range_hosts {
    use super::*;

    #[tokio::test]
    async fn single() {
        let r = IpRange::Single(Ipv4Addr::new(10, 0, 0, 1));
        black_box(r.hosts());
    }

    #[tokio::test]
    async fn cidr_slash30() {
        let r = IpRange::from_str("192.168.0.0/30").unwrap();
        black_box(r.hosts());
    }

    #[tokio::test]
    async fn cidr_slash28() {
        let r = IpRange::from_str("192.168.0.0/28").unwrap();
        black_box(r.hosts());
    }

    #[tokio::test]
    async fn cidr_slash24() {
        let r = IpRange::from_str("10.0.1.0/24").unwrap();
        black_box(r.hosts());
    }

    #[tokio::test]
    async fn cidr_slash16() {
        let r = IpRange::from_str("172.16.0.0/16").unwrap();
        black_box(r.hosts());
    }

    #[tokio::test]
    async fn dash_range_5() {
        let r = IpRange::DashRange(Ipv4Addr::new(10, 0, 0, 1), Ipv4Addr::new(10, 0, 0, 5));
        black_box(r.hosts());
    }

    #[tokio::test]
    async fn dash_range_256() {
        let r = IpRange::DashRange(Ipv4Addr::new(10, 0, 0, 0), Ipv4Addr::new(10, 0, 0, 255));
        black_box(r.hosts());
    }

    #[tokio::test]
    async fn dash_range_1024() {
        let r = IpRange::DashRange(Ipv4Addr::new(10, 0, 0, 0), Ipv4Addr::new(10, 0, 3, 255));
        black_box(r.hosts());
    }
}

mod ip_range_hosts_scaling {
    use super::*;

    #[tokio::test]
    async fn scaling() {
        for n in [1u32, 10, 100, 256, 1_000, 10_000, 65_534] {
            let start = Ipv4Addr::from(0x0A000001u32);
            let end = Ipv4Addr::from(0x0A000001u32 + n - 1);
            let r = IpRange::DashRange(start, end);
            black_box(r.hosts());
        }
    }
}

mod cluman_schema_add {
    use super::*;

    #[tokio::test]
    async fn first_add_from_right() {
        let mut schema = CluManSchema::registration(Ipv4Addr::new(10, 0, 0, 1), Mode::Client);
        schema.add(Ipv4Addr::new(10, 0, 0, 2), Mode::Server);
        black_box(schema);
    }

    #[tokio::test]
    async fn add_to_small_map_10() {
        let mut s = CluManSchema::default();
        for i in 0..10u8 {
            s.add(Ipv4Addr::new(10, 0, 0, i), Mode::Client);
        }
        s.add(Ipv4Addr::new(10, 0, 1, 0), Mode::Client);
        black_box(s);
    }

    #[tokio::test]
    async fn add_to_medium_map_100() {
        let mut s = CluManSchema::default();
        for i in 0u32..100 {
            s.add(Ipv4Addr::from(0x0A000000u32 + i), Mode::Client);
        }
        s.add(Ipv4Addr::new(10, 1, 0, 0), Mode::Client);
        black_box(s);
    }

    #[tokio::test]
    async fn add_to_large_map_1000() {
        let mut s = CluManSchema::default();
        for i in 0u32..1_000 {
            s.add(Ipv4Addr::from(0x0A000000u32 + i), Mode::Client);
        }
        s.add(Ipv4Addr::new(10, 5, 0, 0), Mode::Client);
        black_box(s);
    }
}

mod cluman_schema_add_scaling {
    use super::*;

    #[tokio::test]
    async fn scaling() {
        for n in [1usize, 10, 50, 100, 500] {
            let mut s = CluManSchema::default();
            for i in 0u32..n as u32 {
                s.add(Ipv4Addr::from(0x0A000000u32 + i), Mode::Client);
            }
            black_box(s);
        }
    }
}

mod tasks_single_ops {
    use super::*;

    #[tokio::test]
    async fn push_to_empty() {
        let mut q = Tasks::default();
        q.push(make_task(0));
        black_box(q);
    }

    #[tokio::test]
    async fn push_to_1000() {
        let mut q = Tasks::default();
        for i in 0..1_000 {
            q.push(make_task(i));
        }
        q.push(make_task(9999));
        black_box(q);
    }

    #[tokio::test]
    async fn pop_from_1() {
        let mut q = Tasks::default();
        q.push(make_task(0));
        black_box(q.pop());
    }

    #[tokio::test]
    async fn pop_from_1000() {
        let mut q = Tasks::default();
        for i in 0..1_000 {
            q.push(make_task(i));
        }
        black_box(q.pop());
    }

    #[tokio::test]
    async fn is_empty_false() {
        let mut q = Tasks::default();
        q.push(make_task(0));
        black_box(q.is_empty());
    }

    #[tokio::test]
    async fn len_1000() {
        let mut q = Tasks::default();
        for i in 0..1_000 {
            q.push(make_task(i));
        }
        black_box(q.len());
    }
}

mod tasks_throughput {
    use super::*;

    #[tokio::test]
    async fn push_n() {
        for n in [10usize, 100, 1_000] {
            let mut q = Tasks::default();
            for i in 0..n {
                q.push(make_task(i).await);
            }
            black_box(q);
        }
    }

    #[tokio::test]
    async fn drain_n() {
        for n in [10usize, 100, 1_000] {
            let mut q = Tasks::default();
            for i in 0..n {
                q.push(make_task(i).await);
            }
            while q.pop().is_some() {}
            black_box(q);
        }
    }

    #[tokio::test]
    async fn interleaved_push_pop_1000() {
        let mut q = Tasks::default();
        for i in 0..1_000usize {
            q.push(make_task(i));
            let _ = q.pop();
        }
        black_box(q);
    }
}

mod tasks_new {
    use super::*;

    #[tokio::test]
    async fn from_vec() {
        for n in [10usize, 100, 1_000] {
            let v = (0..n).map(make_task).collect::<Vec<_>>();
            black_box(Tasks::new(v.iter()));
        }
    }
}

mod server_state {
    use super::*;

    #[tokio::test]
    async fn push_task() {
        let s = ServerState::new();
        black_box(s.push_task(make_task(0).await));
    }

    #[tokio::test]
    async fn claim_task_from_nonempty() {
        let s = ServerState::new();
        for i in 0..1_000 {
            s.push_task(make_task(i).await);
        }
        black_box(s.claim_task());
    }

    #[tokio::test]
    async fn claim_task_from_empty() {
        let s = ServerState::new();
        black_box(s.claim_task());
    }

    #[tokio::test]
    async fn register_client() {
        let s = ServerState::new();
        // Fixed IP used in place of fastrand — this is a smoke tokio::test, not a measurement.
        black_box(s.register_client(Ipv4Addr::from(0x0A00_0064_u32)));
    }

    #[tokio::test]
    async fn pending_count() {
        let s = ServerState::new();
        for i in 0..100 {
            s.push_task(make_task(i).await);
        }
        black_box(s.pending_count());
    }

    #[tokio::test]
    async fn client_addrs_100() {
        let s = ServerState::new();
        for i in 0u32..100 {
            s.register_client(Ipv4Addr::from(0x0A000000 + i));
        }
        black_box(s.client_addrs());
    }

    #[tokio::test]
    async fn push_then_claim_roundtrip() {
        let s = ServerState::new();
        s.push_task(make_task(0).await);
        black_box(s.claim_task());
    }
}

mod mode_conversions {
    use super::*;

    #[tokio::test]
    async fn from_str_client() {
        black_box(Mode::from_str("client").unwrap());
    }

    #[tokio::test]
    async fn from_str_server() {
        black_box(Mode::from_str("server").unwrap());
    }

    #[tokio::test]
    async fn from_str_controller() {
        black_box(Mode::from_str("controller").unwrap());
    }

    #[tokio::test]
    async fn from_str_cluman_alias() {
        black_box(Mode::from_str("cluman").unwrap());
    }

    #[tokio::test]
    async fn from_str_err_unknown() {
        black_box(Mode::from_str("unknown").unwrap_err());
    }

    #[tokio::test]
    async fn display_client() {
        black_box(Mode::Client.to_string());
    }

    #[tokio::test]
    async fn display_server() {
        black_box(Mode::Server.to_string());
    }

    #[tokio::test]
    async fn display_controller() {
        black_box(Mode::Controller.to_string());
    }
}

mod task_serde {
    use super::*;

    #[tokio::test]
    async fn serialize_small() {
        let t = make_task(0);
        black_box(serde_json::to_string(&t).unwrap());
    }

    #[tokio::test]
    async fn deserialize_small() {
        let json = serde_json::to_string(&make_task(0)).unwrap();
        black_box(serde_json::from_str::<Task>(&json).unwrap());
    }

    #[tokio::test]
    async fn serialize_large() {
        let t = make_task_large(0);
        black_box(serde_json::to_string(&t).unwrap());
    }

    #[tokio::test]
    async fn deserialize_large() {
        let json = serde_json::to_string(&make_task_large(0)).unwrap();
        black_box(serde_json::from_str::<Task>(&json).unwrap());
    }

    #[tokio::test]
    async fn serialize_tasks_100() {
        let q = Tasks::new((0..100).map(make_task).collect::<Vec<_>>());
        black_box(serde_json::to_string(&q).unwrap());
    }

    #[tokio::test]
    async fn deserialize_tasks_100() {
        let q = Tasks::new((0..100).map(make_task).collect::<Vec<_>>());
        let json = serde_json::to_string(&q).unwrap();
        black_box(serde_json::from_str::<Tasks>(&json).unwrap());
    }
}
