pub mod schema;
pub use schema::UpdMan;

#[cfg(test)]
mod tests {
    use crate::UpdMan;

    fn make_updman(base_url: &str, image_tag: &str, hash: &str) -> UpdMan {
        UpdMan::new(
            base_url.to_string(),
            image_tag.to_string(),
            hash.to_string(),
        )
    }

    // ── image_ref ─────────────────────────────────────────────────────────────

    #[test]
    fn image_ref_combines_base_url_and_image_tag() {
        let u = make_updman(
            "registry.example.com/mtos-v2",
            "util-mdl:latest",
            "sha256:abc123",
        );
        assert_eq!(
            u.image_ref(),
            "registry.example.com/mtos-v2/util-mdl:latest"
        );
    }

    #[test]
    fn image_ref_format_is_base_url_slash_image_tag() {
        let u = make_updman("myregistry.io", "myimage:v0", "sha256:00");
        let expected = format!("{}/{}", "myregistry.io", "myimage:v0");
        assert_eq!(u.image_ref(), expected);
    }

    #[test]
    fn image_ref_with_different_tag() {
        let u = make_updman("reg.io/ns", "app:v2", "sha256:ff");
        assert_eq!(u.image_ref(), "reg.io/ns/app:v2");
    }
}
