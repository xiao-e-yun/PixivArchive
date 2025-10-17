use post_archiver::{PlatformId, importer::UnsyncTag};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PixivTags {
    pub author_id: String,
    pub is_locked: bool,
    pub writable: bool,
    pub tags: Vec<PixivTag>,
}

impl PixivTags {
    pub fn into_tags(&self, platform: PlatformId) -> Vec<UnsyncTag> {
        self.tags
            .iter()
            .map(|tag| {
                let name = tag.tag.clone();

                let is_r18 = matches!(name.as_str(), "R-18" | "R-18G");
                if is_r18 {
                    UnsyncTag { name: name.to_lowercase(), platform: None }
                } else {
                    UnsyncTag { name, platform: Some(platform) }
                }
            })
            .collect()
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PixivTag {
    pub tag: String,
    pub locked: bool,
    pub deletable: bool,
}
