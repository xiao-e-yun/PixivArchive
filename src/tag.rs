use post_archiver::{importer::UnsyncTag, PlatformId};
use serde::Deserialize;
use std::collections::HashMap;

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
                let platform = match name.as_str() {
                    "R-18" | "R-18G" => None,
                    _ => Some(platform),
                };

                UnsyncTag { name, platform }
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
