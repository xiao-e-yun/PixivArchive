use std::{collections::HashMap, path::PathBuf, rc::Rc};

use chrono::{DateTime, Utc};
use log::{debug, error, info};
use post_archiver::{
    Comment,
    importer::{UnsyncCollection, UnsyncContent, UnsyncFileMeta, UnsyncPost},
    manager::PostArchiverManager,
};
use post_archiver_utils::ArchiveClient;
use reqwest::Url;
use serde::Deserialize;
use serde_json::json;
use serde_repr::Deserialize_repr;
use tokio::{
    sync::{
        Mutex,
        mpsc::{UnboundedReceiver, UnboundedSender},
    },
    task::{LocalSet, spawn_local},
};

use crate::{
    config::{Config, ProgressManager},
    fetch,
    file::{ArchiveRequest, PixivUgoira},
    tag::PixivTags,
    user::UserManager,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(untagged)]
pub enum PixivArtworkId {
    Illust(u64),
    Novel(u64),
}

impl PixivArtworkId {
    pub fn id(&self) -> u64 {
        match self {
            PixivArtworkId::Illust(id) => *id,
            PixivArtworkId::Novel(id) => *id,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            PixivArtworkId::Illust(_) => "Illust",
            PixivArtworkId::Novel(_) => "Novel",
        }
    }

    pub fn url(&self) -> String {
        match self {
            PixivArtworkId::Illust(id) => format!("https://www.pixiv.net/artworks/{id}"),
            PixivArtworkId::Novel(id) => format!("https://www.pixiv.net/novel/show.php?id={id}"),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PixivArtwork {
    pub id: String,
    pub title: String,
    pub user_id: String,
    pub user_name: String,
    pub ai_type: AiType,

    pub comment_count: u32,
    pub comment_off: u8,

    pub create_date: String,
    pub upload_date: String,
    pub description: String,

    // pub extra_data: PixivArtworkExtraData, // Ignored for now, this is how to display when share at other platforms
    #[serde(flatten)]
    pub content: PixivArtworkContent,

    pub tags: PixivTags,
    pub series_nav_data: Option<PixivArtworkNavData>,
}

impl PixivArtwork {
    pub fn has_comment(&self) -> bool {
        self.comment_off == 0 && self.comment_count > 0
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all_fields = "camelCase", untagged)]
pub enum PixivArtworkContent {
    Illust {
        illust_comment: String,
        illust_id: String,
        illust_title: String,
        illust_type: IllustType,
    },
    Novel {
        content: String,
        cover_url: String,
    },
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize_repr)]
pub enum AiType {
    Unknown = 0,
    No = 1,
    Yes = 2,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize_repr)]
pub enum IllustType {
    Illust = 0,
    Manga = 1,
    Ugoira = 2,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize_repr)]
pub enum ContentRestrict {
    General = 0,
    R18 = 1,
    R18G = 2,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PixivArtworkNavData {
    series_id: String,
    title: String,
}

impl PixivArtworkNavData {
    pub fn into_collection(&self, user_id: String) -> UnsyncCollection {
        UnsyncCollection::new(
            self.title.clone(),
            format!(
                "https://www.pixiv.net/user/{}/series/{}",
                user_id,
                self.series_id.clone()
            ),
        )
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct PixivIllustPages {
    pub urls: PixivIllustPageUrls,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PixivIllustPageUrls {
    pub original: String,
    pub regular: String,
    pub small: String,
    pub thumb_mini: String,
}

pub async fn resolve_artworks(
    config: &Config,
    manager: PostArchiverManager,
    client: &ArchiveClient,
    mut rx: UnboundedReceiver<PixivArtworkId>,
    tx: UnboundedSender<(PathBuf, ArchiveRequest)>,
) {
    let platform = manager
        .import_platform("pixiv".to_string())
        .expect("Failed to get platform");

    let user_manager = UserManager::new(platform);

    let local_set = LocalSet::new();
    let pb = ProgressManager::new(config.multi.clone(), "artwork");

    let manager = Rc::new(Mutex::new(manager));
    local_set
        .run_until(async move {
            debug!("[artwork] Waiting for artworks to reslove");
            while let Some(artwork) = rx.recv().await {
                let pb = pb.clone();
                pb.inc_length(1);

                let manager = manager.clone();
                let client = client.clone();
                let tx = tx.clone();
                let user_manager = user_manager.clone();
                spawn_local(async move {
                    let requests = reslove_artwork(manager, client, user_manager, artwork).await;

                    info!("[artwork] Archived {} ({})", artwork.name(), artwork.id());
                    pb.inc(1);

                    for request in requests {
                        tx.send(request).unwrap()
                    }
                });
            }
        })
        .await;

    local_set.await;

    info!("[artwork] Archive finished");
}

async fn reslove_artwork(
    manager: Rc<Mutex<PostArchiverManager>>,
    client: ArchiveClient,
    user_manager: UserManager,
    artwork: PixivArtworkId,
) -> Vec<(PathBuf, ArchiveRequest)> {
    let source = artwork.url();

    let artwork = match fetch::<PixivArtwork>(
        &client,
        &match artwork {
            PixivArtworkId::Illust(id) => format!("https://www.pixiv.net/ajax/illust/{id}"),
            PixivArtworkId::Novel(id) => format!("https://www.pixiv.net/ajax/novel/{id}"),
        },
    )
    .await
    {
        Ok(artwork) => artwork,
        Err(e) => {
            error!("[artwork] Failed to fetch {source}: {e:?}");
            return vec![];
        }
    };

    let mut contents = common::parse_description(&artwork);
    let thumb: Option<UnsyncFileMeta<ArchiveRequest>>;

    match &artwork.content {
        PixivArtworkContent::Illust { illust_type, .. } => {
            let file_metas = match illust::fetch_pages(&client, &artwork.id).await {
                Ok(artworks) => artworks,
                Err(e) => {
                    error!("[artwork] Failed to fetch pages {}: {:?}", artwork.id, e);
                    return vec![];
                }
            };
            thumb = file_metas.first().cloned();

            match illust_type {
                IllustType::Illust | IllustType::Manga => {
                    contents.extend(file_metas.into_iter().map(UnsyncContent::File));
                }
                IllustType::Ugoira => {
                    let extra = thumb.as_ref().unwrap().extra.clone();
                    let ugoira = match fetch::<PixivUgoira>(
                        &client,
                        &format!(
                            "https://www.pixiv.net/ajax/illust/{}/ugoira_meta",
                            &artwork.id
                        ),
                    )
                    .await
                    {
                        Ok(ugoira) => ugoira,
                        Err(e) => {
                            error!("[artwork] Failed to fetch ugoira {}: {:?}", artwork.id, e);
                            return vec![];
                        }
                    };

                    contents.push(UnsyncContent::File(
                        UnsyncFileMeta::new(
                            "ugoira.webm".to_string(),
                            "video/webm".to_string(),
                            ArchiveRequest::Ugoira {
                                url: ugoira.original_src,
                                frames: ugoira.frames,
                            },
                        )
                        .extra(extra),
                    ));
                }
            }
        }
        PixivArtworkContent::Novel {
            content, cover_url, ..
        } => {
            contents.push(UnsyncContent::Text(content.clone()));
            thumb = Some(novel::parse_cover(cover_url));
        }
    };

    let comments = common::get_comments(&client, &artwork).await;

    let tags = artwork.tags.into_tags(user_manager.platform);
    let collection = common::get_collections(&artwork).await;

    let mut manager = manager.lock().await;
    let author = user_manager.get(&manager, &artwork).await;

    let tx = manager.transaction().unwrap();
    let (_post, files) =
        match UnsyncPost::new(user_manager.platform, source, artwork.title, contents)
            .thumb(thumb)
            .updated(common::parse_date(&artwork.upload_date))
            .published(common::parse_date(&artwork.create_date))
            .tags(tags)
            .comments(comments)
            .authors(author.into_iter().collect())
            .collections(collection.into_iter().collect())
            .sync(&tx)
        {
            Ok(post) => post,
            Err(e) => {
                error!("[artwork] Failed to sync post {}: {:?}", artwork.id, e);
                return vec![];
            }
        };
    tx.commit().unwrap();

    files
}

mod common {
    use super::*;

    pub fn parse_description(artwork: &PixivArtwork) -> Vec<UnsyncContent<ArchiveRequest>> {
        vec![UnsyncContent::Text(format!(
            "> {}",
            artwork.description.trim().replace('\n', "\n> ")
        ))]
    }

    pub fn parse_date(date: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(date).unwrap().to_utc()
    }

    pub async fn get_comments(client: &ArchiveClient, artwork: &PixivArtwork) -> Vec<Comment> {
        if artwork.has_comment() {
            crate::comment::get_comments(
                client,
                &artwork.id,
                matches!(artwork.content, PixivArtworkContent::Novel { .. }),
                true,
            )
            .await
        } else {
            vec![]
        }
    }

    pub async fn get_collections(artwork: &PixivArtwork) -> Option<UnsyncCollection> {
        // TODO: add more collections support
        artwork
            .series_nav_data
            .as_ref()
            .map(|nav| nav.into_collection(artwork.user_id.clone()))
    }
}

mod illust {
    use post_archiver_utils::Result;

    use super::*;

    pub async fn fetch_pages(
        client: &ArchiveClient,
        artwork_id: &str,
    ) -> Result<Vec<UnsyncFileMeta<ArchiveRequest>>> {
        let pages = fetch::<Vec<PixivIllustPages>>(
            client,
            &format!(
                "https://www.pixiv.net/ajax/illust/{}/pages?lang=ja",
                &artwork_id
            ),
        )
        .await?;

        Ok(pages
            .into_iter()
            .map(|page| {
                url_into_file_meta(page.urls.original, None, None).extra(HashMap::from([
                    ("width".to_string(), json!(page.width)),
                    ("height".to_string(), json!(page.height)),
                ]))
            })
            .collect())
    }
}

mod novel {
    use super::*;

    pub fn parse_cover(url: &str) -> UnsyncFileMeta<ArchiveRequest> {
        url_into_file_meta(
            url.to_string(),
            Some("cover.jpg".to_string()),
            Some((427, 600)),
        )
        .extra(HashMap::from([
            ("width".to_string(), json!(427)),
            ("height".to_string(), json!(600)),
        ]))
    }
}

fn url_into_file_meta(
    url: String,
    filename: Option<String>,
    size: Option<(u32, u32)>,
) -> UnsyncFileMeta<ArchiveRequest> {
    let filename = filename.unwrap_or_else(|| {
        Url::parse(&url)
            .unwrap()
            .path_segments()
            .and_then(|mut segments| segments.next_back())
            .map(|s| s.to_string())
            .unwrap()
    });

    let mime = mime_guess::from_path(&filename).first_or_octet_stream();

    UnsyncFileMeta::new(
        filename,
        mime.to_string(),
        match size {
            Some((width, height)) => ArchiveRequest::ImageWithSize { url, width, height },
            None => ArchiveRequest::Image(url),
        },
    )
}
