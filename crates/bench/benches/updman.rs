//! Smoke tests for the `updman` crate.
//!
//! Exercises:
//! * `UpdMan::new`       — construction at various field string lengths.
//! * `UpdMan::image_ref` — the `format!("{base_url}/{image_tag}")` path.
//! * Combined paths      — construction immediately followed by `image_ref`.
//! * Scaling sweeps      — formatting cost vs URL component length.

use std::hint::black_box;

use updman::UpdMan;

const BASE_URL_MINIMAL: &str = "localhost";
const IMAGE_TAG_MINIMAL: &str = "app:v1";
const HASH_MINIMAL: &str = "sha256:aa";

const BASE_URL_SHORT: &str = "registry.example.com";
const IMAGE_TAG_SHORT: &str = "util-mdl:latest";
const HASH_SHORT: &str = "sha256:deadbeef";

const BASE_URL_MEDIUM: &str = "registry.example.com/losos-linux";
const IMAGE_TAG_MEDIUM: &str = "util-mdl:1.2.3";
const HASH_MEDIUM: &str = "sha256:deadbeefcafe0123456789abcdef0123456789abcdef0123456789abcdef0123";

const BASE_URL_LONG: &str =
    "registry.gitlab.com/organisation/group/subgroup/project/images/losos-linux";
const IMAGE_TAG_LONG: &str = "util-mdl-x86_64-musl:1.23.456-rc.7+build.20240101";
const HASH_LONG: &str =
    "sha256:aaaaaabbbbbbccccccddddddeeeeeeffffffgggggghhhhhhiiiiiijjjjjjkkkkkkllllll";

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

mod new {
    use super::*;

    #[test]
    fn minimal() {
        black_box(UpdMan::new(
            BASE_URL_MINIMAL.to_owned(),
            IMAGE_TAG_MINIMAL.to_owned(),
            HASH_MINIMAL.to_owned(),
        ));
    }

    #[test]
    fn short() {
        black_box(UpdMan::new(
            BASE_URL_SHORT.to_owned(),
            IMAGE_TAG_SHORT.to_owned(),
            HASH_SHORT.to_owned(),
        ));
    }

    #[test]
    fn medium() {
        black_box(UpdMan::new(
            BASE_URL_MEDIUM.to_owned(),
            IMAGE_TAG_MEDIUM.to_owned(),
            HASH_MEDIUM.to_owned(),
        ));
    }

    #[test]
    fn long() {
        black_box(UpdMan::new(
            BASE_URL_LONG.to_owned(),
            IMAGE_TAG_LONG.to_owned(),
            HASH_LONG.to_owned(),
        ));
    }

    #[test]
    fn very_long() {
        black_box(UpdMan::new(
            BASE_URL_VERY_LONG.to_owned(),
            IMAGE_TAG_VERY_LONG.to_owned(),
            HASH_VERY_LONG.to_owned(),
        ));
    }
}

mod image_ref {
    use super::*;

    #[test]
    fn minimal() {
        let u = UpdMan::new(
            BASE_URL_MINIMAL.to_owned(),
            IMAGE_TAG_MINIMAL.to_owned(),
            HASH_MINIMAL.to_owned(),
        );
        black_box(u.image_ref());
    }

    #[test]
    fn short() {
        let u = UpdMan::new(
            BASE_URL_SHORT.to_owned(),
            IMAGE_TAG_SHORT.to_owned(),
            HASH_SHORT.to_owned(),
        );
        black_box(u.image_ref());
    }

    #[test]
    fn medium() {
        let u = UpdMan::new(
            BASE_URL_MEDIUM.to_owned(),
            IMAGE_TAG_MEDIUM.to_owned(),
            HASH_MEDIUM.to_owned(),
        );
        black_box(u.image_ref());
    }

    #[test]
    fn long() {
        let u = UpdMan::new(
            BASE_URL_LONG.to_owned(),
            IMAGE_TAG_LONG.to_owned(),
            HASH_LONG.to_owned(),
        );
        black_box(u.image_ref());
    }

    #[test]
    fn very_long() {
        let u = UpdMan::new(
            BASE_URL_VERY_LONG.to_owned(),
            IMAGE_TAG_VERY_LONG.to_owned(),
            HASH_VERY_LONG.to_owned(),
        );
        black_box(u.image_ref());
    }
}

mod image_ref_double_call {
    use super::*;

    #[test]
    fn short() {
        let u = UpdMan::new(
            BASE_URL_SHORT.to_owned(),
            IMAGE_TAG_SHORT.to_owned(),
            HASH_SHORT.to_owned(),
        );
        black_box(u.image_ref());
        black_box(u.image_ref());
    }

    #[test]
    fn medium() {
        let u = UpdMan::new(
            BASE_URL_MEDIUM.to_owned(),
            IMAGE_TAG_MEDIUM.to_owned(),
            HASH_MEDIUM.to_owned(),
        );
        black_box(u.image_ref());
        black_box(u.image_ref());
    }

    #[test]
    fn long() {
        let u = UpdMan::new(
            BASE_URL_LONG.to_owned(),
            IMAGE_TAG_LONG.to_owned(),
            HASH_LONG.to_owned(),
        );
        black_box(u.image_ref());
        black_box(u.image_ref());
    }
}

mod new_then_image_ref {
    use super::*;

    #[test]
    fn minimal() {
        black_box(
            UpdMan::new(
                BASE_URL_MINIMAL.to_owned(),
                IMAGE_TAG_MINIMAL.to_owned(),
                HASH_MINIMAL.to_owned(),
            )
            .image_ref(),
        );
    }

    #[test]
    fn short() {
        black_box(
            UpdMan::new(
                BASE_URL_SHORT.to_owned(),
                IMAGE_TAG_SHORT.to_owned(),
                HASH_SHORT.to_owned(),
            )
            .image_ref(),
        );
    }

    #[test]
    fn medium() {
        black_box(
            UpdMan::new(
                BASE_URL_MEDIUM.to_owned(),
                IMAGE_TAG_MEDIUM.to_owned(),
                HASH_MEDIUM.to_owned(),
            )
            .image_ref(),
        );
    }

    #[test]
    fn long() {
        black_box(
            UpdMan::new(
                BASE_URL_LONG.to_owned(),
                IMAGE_TAG_LONG.to_owned(),
                HASH_LONG.to_owned(),
            )
            .image_ref(),
        );
    }

    #[test]
    fn very_long() {
        black_box(
            UpdMan::new(
                BASE_URL_VERY_LONG.to_owned(),
                IMAGE_TAG_VERY_LONG.to_owned(),
                HASH_VERY_LONG.to_owned(),
            )
            .image_ref(),
        );
    }
}

mod image_ref_base_url_scaling {
    use super::*;

    #[test]
    fn scaling() {
        let fixed_tag = "util-mdl:latest".to_owned();
        let fixed_hash = "sha256:cafe".to_owned();
        for len in [8usize, 16, 32, 64, 128, 192, 256] {
            let base: String = format!(
                "r.example.com/{}",
                "a".repeat(len.saturating_sub("r.example.com/".len()))
            );
            let u = UpdMan::new(base, fixed_tag.clone(), fixed_hash.clone());
            black_box(u.image_ref());
        }
    }
}

mod image_ref_image_tag_scaling {
    use super::*;

    #[test]
    fn scaling() {
        let fixed_base = "registry.example.com/mtos".to_owned();
        let fixed_hash = "sha256:cafe".to_owned();
        for len in [4usize, 8, 16, 32, 64, 96, 128] {
            let tag: String = format!("app:{}", "v".repeat(len.saturating_sub("app:".len())));
            let u = UpdMan::new(fixed_base.clone(), tag, fixed_hash.clone());
            black_box(u.image_ref());
        }
    }
}

mod image_ref_total_len_scaling {
    use super::*;

    #[test]
    fn scaling() {
        for total in [20usize, 40, 80, 128, 200, 300, 512] {
            let base_len = (total * 7) / 10;
            let tag_len = total - base_len - 1;
            let base: String = format!(
                "r.io/{}",
                "x".repeat(base_len.saturating_sub("r.io/".len()))
            );
            let tag: String = format!("i:{}", "t".repeat(tag_len.saturating_sub("i:".len())));
            let u = UpdMan::new(base, tag, "sha256:00".to_owned());
            black_box(u.image_ref());
        }
    }
}

mod image_ref_fresh_strings {
    use super::*;

    #[test]
    fn medium_fresh() {
        let u = UpdMan::new(
            BASE_URL_MEDIUM.to_owned(),
            IMAGE_TAG_MEDIUM.to_owned(),
            HASH_MEDIUM.to_owned(),
        );
        black_box(u.image_ref());
    }

    #[test]
    fn long_fresh() {
        let u = UpdMan::new(
            BASE_URL_LONG.to_owned(),
            IMAGE_TAG_LONG.to_owned(),
            HASH_LONG.to_owned(),
        );
        black_box(u.image_ref());
    }
}
