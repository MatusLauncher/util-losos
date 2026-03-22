//! Benchmarks for the `updman` crate.
//!
//! Covers:
//! * `UpdMan::new`      — construction cost at various field string lengths.
//! * `UpdMan::image_ref` — the `format!("{base_url}/{image_tag}")` hot path,
//!                         which is called for every `nerdctl pull` and
//!                         `nerdctl save` invocation.
//! * Combined paths     — construction immediately followed by `image_ref`,
//!                         mirroring the real call sequence in `update()`.
//! * Scaling sweeps     — how formatting time grows with URL component length.

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use updman::UpdMan;

// ── realistic fixtures ────────────────────────────────────────────────────────

/// Minimal registry reference — bare hostname, no namespace.
const BASE_URL_MINIMAL: &str = "localhost";
const IMAGE_TAG_MINIMAL: &str = "app:v1";
const HASH_MINIMAL: &str = "sha256:aa";

/// Typical single-level registry with a short tag.
const BASE_URL_SHORT: &str = "registry.example.com";
const IMAGE_TAG_SHORT: &str = "util-mdl:latest";
const HASH_SHORT: &str = "sha256:deadbeef";

/// Realistic production registry: host + one namespace component.
const BASE_URL_MEDIUM: &str = "registry.example.com/mtos-v2";
const IMAGE_TAG_MEDIUM: &str = "util-mdl:1.2.3";
const HASH_MEDIUM: &str = "sha256:deadbeefcafe0123456789abcdef0123456789abcdef0123456789abcdef0123";

/// Deep multi-segment registry path (e.g. GitLab Container Registry).
const BASE_URL_LONG: &str =
    "registry.gitlab.com/organisation/group/subgroup/project/images/mtos-v2";
const IMAGE_TAG_LONG: &str = "util-mdl-x86_64-musl:1.23.456-rc.7+build.20240101";
const HASH_LONG: &str =
    "sha256:aaaaaabbbbbbccccccddddddeeeeeeffffffgggggghhhhhhiiiiiijjjjjjkkkkkkllllll";

/// Very long base URL — stresses the formatter with heap allocation.
const BASE_URL_VERY_LONG: &str = concat!(
    "registry.very-long-hostname-that-exceeds-typical-dns-label-limits.internal.corporate.example",
    ".com/some/very/deeply/nested/image/namespace/hierarchy/for/a/large/organisation/mtos"
);
const IMAGE_TAG_VERY_LONG: &str =
    "util-mdl-x86_64-unknown-linux-musl:2.100.999-alpha.42+git.abcdef1234567890.20241231";
const HASH_VERY_LONG: &str = concat!(
    "sha256:",
    "0000000000000000000000000000000000000000000000000000000000000000",
    "1111111111111111111111111111111111111111111111111111111111111111"
);

// ── UpdMan::new ───────────────────────────────────────────────────────────────

fn bench_new(c: &mut Criterion) {
    let mut group = c.benchmark_group("updman/new");

    group.bench_function("minimal", |b| {
        b.iter(|| {
            UpdMan::new(
                BASE_URL_MINIMAL.to_owned(),
                IMAGE_TAG_MINIMAL.to_owned(),
                HASH_MINIMAL.to_owned(),
            )
        });
    });

    group.bench_function("short", |b| {
        b.iter(|| {
            UpdMan::new(
                BASE_URL_SHORT.to_owned(),
                IMAGE_TAG_SHORT.to_owned(),
                HASH_SHORT.to_owned(),
            )
        });
    });

    group.bench_function("medium", |b| {
        b.iter(|| {
            UpdMan::new(
                BASE_URL_MEDIUM.to_owned(),
                IMAGE_TAG_MEDIUM.to_owned(),
                HASH_MEDIUM.to_owned(),
            )
        });
    });

    group.bench_function("long", |b| {
        b.iter(|| {
            UpdMan::new(
                BASE_URL_LONG.to_owned(),
                IMAGE_TAG_LONG.to_owned(),
                HASH_LONG.to_owned(),
            )
        });
    });

    group.bench_function("very_long", |b| {
        b.iter(|| {
            UpdMan::new(
                BASE_URL_VERY_LONG.to_owned(),
                IMAGE_TAG_VERY_LONG.to_owned(),
                HASH_VERY_LONG.to_owned(),
            )
        });
    });

    group.finish();
}

// ── UpdMan::image_ref ─────────────────────────────────────────────────────────

fn bench_image_ref(c: &mut Criterion) {
    let mut group = c.benchmark_group("updman/image_ref");

    // Pre-construct each UpdMan so the benchmark measures only image_ref().
    let minimal = UpdMan::new(
        BASE_URL_MINIMAL.to_owned(),
        IMAGE_TAG_MINIMAL.to_owned(),
        HASH_MINIMAL.to_owned(),
    );
    let short = UpdMan::new(
        BASE_URL_SHORT.to_owned(),
        IMAGE_TAG_SHORT.to_owned(),
        HASH_SHORT.to_owned(),
    );
    let medium = UpdMan::new(
        BASE_URL_MEDIUM.to_owned(),
        IMAGE_TAG_MEDIUM.to_owned(),
        HASH_MEDIUM.to_owned(),
    );
    let long = UpdMan::new(
        BASE_URL_LONG.to_owned(),
        IMAGE_TAG_LONG.to_owned(),
        HASH_LONG.to_owned(),
    );
    let very_long = UpdMan::new(
        BASE_URL_VERY_LONG.to_owned(),
        IMAGE_TAG_VERY_LONG.to_owned(),
        HASH_VERY_LONG.to_owned(),
    );

    group.bench_function("minimal", |b| {
        b.iter(|| minimal.image_ref());
    });

    group.bench_function("short", |b| {
        b.iter(|| short.image_ref());
    });

    group.bench_function("medium", |b| {
        b.iter(|| medium.image_ref());
    });

    group.bench_function("long", |b| {
        b.iter(|| long.image_ref());
    });

    group.bench_function("very_long", |b| {
        b.iter(|| very_long.image_ref());
    });

    group.finish();
}

// ── image_ref called repeatedly on the same instance ─────────────────────────
//
// In practice update() calls image_ref() twice (once for `nerdctl pull`,
// once for `nerdctl save`). Measure the cost of two back-to-back calls.

fn bench_image_ref_double_call(c: &mut Criterion) {
    let mut group = c.benchmark_group("updman/image_ref_double_call");

    let short = UpdMan::new(
        BASE_URL_SHORT.to_owned(),
        IMAGE_TAG_SHORT.to_owned(),
        HASH_SHORT.to_owned(),
    );
    let medium = UpdMan::new(
        BASE_URL_MEDIUM.to_owned(),
        IMAGE_TAG_MEDIUM.to_owned(),
        HASH_MEDIUM.to_owned(),
    );
    let long = UpdMan::new(
        BASE_URL_LONG.to_owned(),
        IMAGE_TAG_LONG.to_owned(),
        HASH_LONG.to_owned(),
    );

    group.bench_function("short", |b| {
        b.iter(|| {
            let _a = short.image_ref();
            let _b = short.image_ref();
        });
    });

    group.bench_function("medium", |b| {
        b.iter(|| {
            let _a = medium.image_ref();
            let _b = medium.image_ref();
        });
    });

    group.bench_function("long", |b| {
        b.iter(|| {
            let _a = long.image_ref();
            let _b = long.image_ref();
        });
    });

    group.finish();
}

// ── combined new + image_ref ──────────────────────────────────────────────────
//
// Models the cold-start path: `UpdMan` is constructed from the kernel command
// line and immediately used to build the registry reference.

fn bench_new_then_image_ref(c: &mut Criterion) {
    let mut group = c.benchmark_group("updman/new_then_image_ref");

    group.bench_function("minimal", |b| {
        b.iter(|| {
            UpdMan::new(
                BASE_URL_MINIMAL.to_owned(),
                IMAGE_TAG_MINIMAL.to_owned(),
                HASH_MINIMAL.to_owned(),
            )
            .image_ref()
        });
    });

    group.bench_function("short", |b| {
        b.iter(|| {
            UpdMan::new(
                BASE_URL_SHORT.to_owned(),
                IMAGE_TAG_SHORT.to_owned(),
                HASH_SHORT.to_owned(),
            )
            .image_ref()
        });
    });

    group.bench_function("medium", |b| {
        b.iter(|| {
            UpdMan::new(
                BASE_URL_MEDIUM.to_owned(),
                IMAGE_TAG_MEDIUM.to_owned(),
                HASH_MEDIUM.to_owned(),
            )
            .image_ref()
        });
    });

    group.bench_function("long", |b| {
        b.iter(|| {
            UpdMan::new(
                BASE_URL_LONG.to_owned(),
                IMAGE_TAG_LONG.to_owned(),
                HASH_LONG.to_owned(),
            )
            .image_ref()
        });
    });

    group.bench_function("very_long", |b| {
        b.iter(|| {
            UpdMan::new(
                BASE_URL_VERY_LONG.to_owned(),
                IMAGE_TAG_VERY_LONG.to_owned(),
                HASH_VERY_LONG.to_owned(),
            )
            .image_ref()
        });
    });

    group.finish();
}

// ── image_ref scaling — base_url length ──────────────────────────────────────
//
// Holds `image_tag` constant and sweeps `base_url` from 8 to 256 characters.
// Shows exactly how the heap-allocation cost in `format!` scales with the
// longer of the two string components.

fn bench_image_ref_base_url_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("updman/image_ref_base_url_scaling");

    let fixed_tag = "util-mdl:latest".to_owned();
    let fixed_hash = "sha256:cafe".to_owned();

    for len in [8usize, 16, 32, 64, 128, 192, 256] {
        // Build a base_url of exactly `len` characters: "r.example.com/aaa…"
        let base: String = format!(
            "r.example.com/{}",
            "a".repeat(len.saturating_sub("r.example.com/".len()))
        );
        let u = UpdMan::new(base, fixed_tag.clone(), fixed_hash.clone());

        group.bench_with_input(BenchmarkId::from_parameter(len), &u, |b, updman| {
            b.iter(|| updman.image_ref());
        });
    }

    group.finish();
}

// ── image_ref scaling — image_tag length ─────────────────────────────────────
//
// Holds `base_url` constant and sweeps `image_tag` from 4 to 128 characters.

fn bench_image_ref_image_tag_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("updman/image_ref_image_tag_scaling");

    let fixed_base = "registry.example.com/mtos".to_owned();
    let fixed_hash = "sha256:cafe".to_owned();

    for len in [4usize, 8, 16, 32, 64, 96, 128] {
        // Build an image_tag of exactly `len` characters: "app:vXXX…"
        let tag: String = format!("app:{}", "v".repeat(len.saturating_sub("app:".len())));
        let u = UpdMan::new(fixed_base.clone(), tag, fixed_hash.clone());

        group.bench_with_input(BenchmarkId::from_parameter(len), &u, |b, updman| {
            b.iter(|| updman.image_ref());
        });
    }

    group.finish();
}

// ── image_ref scaling — total reference length ────────────────────────────────
//
// Sweeps both components together so the total reference string grows from
// ~20 to ~512 characters. Directly models the heap-allocation cost of
// `format!("{base_url}/{image_tag}")` as a function of output length.

fn bench_image_ref_total_len_scaling(c: &mut Criterion) {
    let mut group = c.benchmark_group("updman/image_ref_total_len_scaling");

    // Target total output lengths (base_url/image_tag); split roughly 70/30.
    for total in [20usize, 40, 80, 128, 200, 300, 512] {
        let base_len = (total * 7) / 10; // ~70 %
        let tag_len = total - base_len - 1; // -1 for the '/' separator

        let base: String = format!(
            "r.io/{}",
            "x".repeat(base_len.saturating_sub("r.io/".len()))
        );
        let tag: String = format!("i:{}", "t".repeat(tag_len.saturating_sub("i:".len())));
        let u = UpdMan::new(base, tag, "sha256:00".to_owned());

        group.bench_with_input(BenchmarkId::from_parameter(total), &u, |b, updman| {
            b.iter(|| updman.image_ref());
        });
    }

    group.finish();
}

// ── BatchSize::SmallInput variant ─────────────────────────────────────────────
//
// Uses iter_batched so Criterion allocates fresh Strings each iteration,
// confirming the benchmark is not affected by String interning or alias
// analysis across calls.

fn bench_image_ref_fresh_strings(c: &mut Criterion) {
    let mut group = c.benchmark_group("updman/image_ref_fresh_strings");

    group.bench_function("medium_fresh", |b| {
        b.iter_batched(
            || {
                UpdMan::new(
                    BASE_URL_MEDIUM.to_owned(),
                    IMAGE_TAG_MEDIUM.to_owned(),
                    HASH_MEDIUM.to_owned(),
                )
            },
            |u| u.image_ref(),
            criterion::BatchSize::SmallInput,
        );
    });

    group.bench_function("long_fresh", |b| {
        b.iter_batched(
            || {
                UpdMan::new(
                    BASE_URL_LONG.to_owned(),
                    IMAGE_TAG_LONG.to_owned(),
                    HASH_LONG.to_owned(),
                )
            },
            |u| u.image_ref(),
            criterion::BatchSize::SmallInput,
        );
    });

    group.finish();
}

// ── criterion wiring ──────────────────────────────────────────────────────────

criterion_group!(
    benches,
    bench_new,
    bench_image_ref,
    bench_image_ref_double_call,
    bench_new_then_image_ref,
    bench_image_ref_base_url_scaling,
    bench_image_ref_image_tag_scaling,
    bench_image_ref_total_len_scaling,
    bench_image_ref_fresh_strings,
);
criterion_main!(benches);
