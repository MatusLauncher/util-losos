use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct UpdMan {
    base_url: String,
    image_tag: String,
}

impl UpdMan {
    pub fn update(&self) -> miette::Result<()> {
        Ok(())
    }
}
