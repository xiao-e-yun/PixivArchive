use std::{collections::HashMap, path::PathBuf};

use chrono::{DateTime, Utc};
use futures::try_join;
use log::{error, info, trace};
use plyne::{Input, Output};
use post_archiver::{
    Comment,
    importer::{UnsyncCollection, UnsyncContent, UnsyncFileMeta, UnsyncPost},
};
use post_archiver_utils::{ArchiveClient, Result};
use reqwest::Url;
use serde::Deserialize;
use serde_json::json;
use serde_repr::Deserialize_repr;
use tempfile::TempPath;
use tokio::{
    fs::{self, File, OpenOptions, create_dir_all},
    io, join,
    task::JoinSet,
};

use crate::{
    FileEvent, Manager, SyncEvent,
    config::{Config, Progress},
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

    pub fn api_url(&self) -> String {
        match self {
            PixivArtworkId::Illust(id) => format!("https://www.pixiv.net/ajax/illust/{id}"),
            PixivArtworkId::Novel(id) => format!("https://www.pixiv.net/ajax/novel/{id}"),
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
    mut artworks_pipeline: Output<PixivArtworkId>,
    files_pipeline: Input<FileEvent>,
    sync_pipeline: Input<SyncEvent>,
    client: &ArchiveClient,
    manager: &Manager,
    config: &Config,
) {
    let pb = Progress::new(config.multi.clone(), "artwork");
    let mut tasks = JoinSet::new();
    while let Some(id) = artworks_pipeline.recv().await {
        pb.inc_length(1);
        if matches!(manager.lock().await.find_post(&id.url()), Ok(Some(_))) {
            info!("[artwork] Skipping existing post: {}", id.url());
            pb.inc(1);
            continue;
        }

        let (tx, rx) = tokio::sync::oneshot::channel();
        let files_pipeline = files_pipeline.clone();
        let sync_pipeline = sync_pipeline.clone();
        let client = client.clone();
        let pb = pb.clone();
        tasks.spawn(async move {
            let source = id.url();

            let artwork = match fetch::<PixivArtwork>(&client, &id.api_url()).await {
                Ok(artwork) => artwork,
                Err(e) => {
                    error!("[artwork] Failed to fetch {source}: {e:?}");
                    return;
                }
            };

            let ((contents, thumb), comments) = join!(
                common::get_contents_and_thumb(&client, &artwork),
                common::get_comments(&client, &artwork)
            );

            let files = contents
                .iter()
                .filter_map(|c| match c {
                    UnsyncContent::File(f) => Some(f),
                    UnsyncContent::Text(_) => None,
                })
                .chain(thumb.iter())
                .map(|f| f.data.clone())
                .collect::<Vec<_>>();

            files_pipeline.send((files, tx)).unwrap();
            sync_pipeline
                .send(SyncEvent {
                    source,
                    artwork,
                    contents,
                    thumb,
                    comments,
                    files: rx,
                })
                .unwrap();

            pb.inc(1);
        });
    }

    tasks.join_all().await;
    info!("[artwork] Archive resolved");
}

pub async fn archive_artworks(mut sync_pipeline: Output<SyncEvent>, manager: &Manager) {
    let platform = manager
        .lock()
        .await
        .import_platform("pixiv".to_string())
        .expect("Failed to get platform");

    let mut user_manager = UserManager::new(platform);

    'main: while let Some(event) = sync_pipeline.recv().await {
        let Ok(mut files_map) = event.files.await else {
            error!("[artwork] Failed to archive files for {}", event.artwork.id);
            continue;
        };

        let Ok(author) = user_manager.import(&manager.lock().await, &event.artwork) else {
            error!(
                "[artwork] Failed to archive author for {}",
                event.artwork.user_id
            );
            continue;
        };

        let mut manager = manager.lock().await;
        let manager = manager.transaction().unwrap();
        let files = match UnsyncPost::new(
            platform,
            event.source,
            event.artwork.title.clone(),
            event.contents,
        )
        .thumb(event.thumb)
        .authors(vec![author])
        .comments(event.comments)
        .published(common::parse_date(&event.artwork.create_date))
        .updated(common::parse_date(&event.artwork.upload_date))
        .tags(event.artwork.tags.into_tags(platform))
        .collections(common::get_collections(&event.artwork))
        .sync(&manager)
        {
            Ok((_post, files_map)) => files_map,
            Err(e) => {
                error!(
                    "[artwork] Failed to archive post for {}: {:?}",
                    event.artwork.id, e
                );
                continue;
            }
        };

        if let Some(path) = files.first().map(|(dst, _)| dst.parent().unwrap())
            && let Err(e) = fs::create_dir_all(path).await
        {
            error!(
                "[artwork] Failed to create directory for {}: {}",
                path.display(),
                e
            );
            continue;
        }

        let mut create_dir = true;
        for (path, req) in files {
            let url = req.url().to_string();
            if let Err(e) = save_file(&mut files_map, &path, &url, create_dir).await {
                error!("[artwork] Failed to save file {}: {}", path.display(), e);
                continue 'main;
            };
            create_dir = false;
        }

        if let Err(e) = manager.commit() {
            error!(
                "[artwork] Failed to commit transaction for {}: {e:?}",
                event.artwork.id
            );
            continue;
        }
        info!(
            "[artwork] Archived {} ({})",
            event.artwork.title, event.artwork.id
        );
    }

    async fn save_file(
        file_map: &mut HashMap<String, TempPath>,
        path: &PathBuf,
        url: &str,
        create_dir: bool,
    ) -> Result<()> {
        if create_dir {
            let path = path.parent().unwrap();
            create_dir_all(path).await?;
        }

        let temp = file_map.remove(url).ok_or(io::Error::new(
            io::ErrorKind::NotFound,
            format!("File not found in map: {url}"),
        ))?;

        let mut open_options = OpenOptions::new();
        let (mut src, mut dst) = try_join!(
            File::open(&temp),
            open_options
                .create(true)
                .write(true)
                .truncate(true)
                .open(&path)
        )?;

        io::copy(&mut src, &mut dst).await?;
        trace!("File saved: {url} -> {}", path.display());

        Ok(())
    }

    info!("[artwork] Archive finished");
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

    pub fn get_collections(artwork: &PixivArtwork) -> Vec<UnsyncCollection> {
        // TODO: add more collections support
        artwork
            .series_nav_data
            .as_ref()
            .map(|nav| nav.into_collection(artwork.user_id.clone()))
            .into_iter()
            .collect()
    }

    pub async fn get_contents_and_thumb(
        client: &ArchiveClient,
        artwork: &PixivArtwork,
    ) -> (
        Vec<UnsyncContent<ArchiveRequest>>,
        Option<UnsyncFileMeta<ArchiveRequest>>,
    ) {
        let mut contents = common::parse_description(artwork);
        let thumb: Option<UnsyncFileMeta<ArchiveRequest>>;

        match &artwork.content {
            PixivArtworkContent::Illust { illust_type, .. } => {
                let file_metas = match illust::fetch_pages(client, &artwork.id).await {
                    Ok(artworks) => artworks,
                    Err(e) => {
                        error!("[artwork] Failed to fetch pages {}: {:?}", artwork.id, e);
                        return (vec![], None);
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
                            client,
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
                                return (vec![], None);
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

        (contents, thumb)
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
