use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use chrono::{DateTime, Utc};
use futures::future::try_join_all;
use log::{debug, error, info, warn};
use plyne::Output;
use post_archiver::{
    Comment, PlatformId,
    importer::{
        UnsyncAlias, UnsyncAuthor, UnsyncCollection, UnsyncContent, UnsyncFileMeta, UnsyncPost,
    },
};
use post_archiver_utils::{Result, Strategy, TaskSet};
use reqwest::Url;
use serde::Deserialize;
use serde_json::{Value, json};
use serde_repr::Deserialize_repr;
use tempfile::tempdir;
use tokio::join;

use crate::{
    Manager,
    api::{PixivClient, PixivDownloader},
    config::{Config, IncOnDrop, ProgressSet},
    file::{ArchiveRequest, PixivUgoira, download_file},
    tag::PixivTags,
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

    pub fn is_ugoira(&self) -> bool {
        matches!(
            self.content,
            PixivArtworkContent::Illust {
                illust_type: IllustType::Ugoira,
                ..
            }
        )
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

#[serde_with::serde_as]
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PixivArtworkNavData {
    /// A string for illust series, a number for novel series.
    #[serde_as(as = "serde_with::PickFirst<(serde_with::DisplayFromStr, _)>")]
    series_id: u64,
    title: String,
}

impl PixivArtworkNavData {
    pub fn into_collection(&self, user_id: &str, is_novel: bool) -> UnsyncCollection {
        let source = if is_novel {
            format!("https://www.pixiv.net/novel/series/{}", self.series_id)
        } else {
            format!("https://www.pixiv.net/user/{user_id}/series/{}", self.series_id)
        };
        UnsyncCollection::new(self.title.clone(), source)
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

/// A file to download, with the metadata it will be imported under.
#[derive(Debug, Clone)]
pub struct PendingFile {
    pub filename: String,
    pub mime: String,
    pub extra: HashMap<String, Value>,
    pub request: ArchiveRequest,
}

impl PendingFile {
    fn extra(mut self, extra: HashMap<String, Value>) -> Self {
        self.extra = extra;
        self
    }
}

#[derive(Debug, Clone)]
pub enum PendingContent {
    Text(String),
    File(PendingFile),
}

pub async fn artworks(
    mut artworks_pipeline: Output<PixivArtworkId>,
    client: &PixivClient,
    manager: &Manager,
    config: &Config,
    platform: &PlatformId,
    progress: &ProgressSet,
) {
    let tasks: TaskSet<(), ()> = TaskSet::new("artworks").limit(4);
    let downloader = PixivDownloader::new(config, progress.files.clone());

    // The same artwork can arrive from users, series and favorites.
    let mut seen = HashSet::new();

    while let Some(id) = artworks_pipeline.recv().await {
        progress.artworks.inc_length(1);
        let guard = IncOnDrop(progress.artworks.clone());

        if !seen.insert(id) {
            continue;
        }

        if matches!(config.strategy, Strategy::Increment) {
            let mut manager = manager.lock().await;
            if let Ok(Some(_)) = manager.importer().lookup_post(&id.url()) {
                debug!("[artwork] Skipping existing post: {}", id.url());
                continue;
            }
        }

        tasks
            .spawn(artwork_task(
                id,
                client.clone(),
                downloader.clone(),
                manager.clone(),
                *platform,
                config.has_ffmpeg,
                guard,
            ))
            .await;
    }

    tasks.join_all().await;
    progress.artworks.finish();
    progress.files.finish();
    info!("[artwork] Archive finished");
}

async fn artwork_task(
    id: PixivArtworkId,
    client: PixivClient,
    downloader: PixivDownloader,
    manager: Manager,
    platform: PlatformId,
    has_ffmpeg: bool,
    _guard: IncOnDrop,
) -> std::result::Result<(), ()> {
    let source = id.url();

    let artwork = client
        .fetch::<PixivArtwork>(&id.api_url())
        .await
        .map_err(|e| error!("[artwork] Failed to fetch {source}: {e}"))?;

    if !has_ffmpeg && artwork.is_ugoira() {
        warn!(
            "[artwork] Skipping Ugoira {} because ffmpeg is not found",
            artwork.id
        );
        return Err(());
    }

    let (contents, comments) = join!(
        common::get_contents_and_thumb(&client, &artwork),
        common::get_comments(&client, &artwork)
    );
    let (contents, thumb) =
        contents.map_err(|e| error!("[artwork] Failed to resolve {source}: {e}"))?;

    let temp_dir = tempdir().map_err(|e| error!("[artwork] Failed to create temp dir: {e}"))?;
    let (contents, thumb) = download_contents(contents, thumb, &downloader, temp_dir.path())
        .await
        .map_err(|e| error!("[artwork] Failed to download files for {source}: {e}"))?;

    let post = UnsyncPost::new("pixiv".to_string(), source.clone(), artwork.title.clone(), contents)
        .thumb(thumb)
        .authors(vec![common::get_author(&artwork)])
        .comments(comments)
        .published(common::parse_date(&artwork.create_date))
        .updated(common::parse_date(&artwork.upload_date))
        .tags(artwork.tags.into_tags(platform))
        .collections(common::get_collections(&artwork));

    // `import_post` copies files with blocking I/O; keep it off the async runtime.
    // `temp_dir` must outlive the import, since the files are copied from it.
    let result = tokio::task::spawn_blocking(move || {
        let result = manager.blocking_lock().importer().import_post(post);
        drop(temp_dir);
        result
    })
    .await;

    match result {
        Ok(Ok(_)) => {
            info!("[artwork] Archived {} ({})", artwork.title, artwork.id);
            Ok(())
        }
        Ok(Err(e)) => {
            error!("[artwork] Failed to import {source}: {e}");
            Err(())
        }
        Err(e) => {
            error!("[artwork] Import task panicked for {source}: {e}");
            Err(())
        }
    }
}

/// Download every pending file once (the thumbnail usually repeats the first page),
/// then turn the pending contents into importable ones.
async fn download_contents(
    contents: Vec<PendingContent>,
    thumb: Option<PendingFile>,
    downloader: &PixivDownloader,
    with_in: &Path,
) -> Result<(Vec<UnsyncContent>, Option<UnsyncFileMeta>)> {
    let mut requests: HashMap<String, ArchiveRequest> = HashMap::new();
    for file in contents
        .iter()
        .filter_map(|c| match c {
            PendingContent::File(f) => Some(f),
            PendingContent::Text(_) => None,
        })
        .chain(thumb.iter())
    {
        requests
            .entry(file.request.url().to_string())
            .or_insert_with(|| file.request.clone());
    }

    let downloaded: HashMap<String, PathBuf> =
        try_join_all(requests.into_iter().map(|(url, request)| async move {
            let path = download_file(request, downloader, with_in).await?;
            Ok::<_, post_archiver_utils::Error>((url, path))
        }))
        .await?
        .into_iter()
        .collect();

    let into_meta = |file: PendingFile| {
        let source = downloaded[file.request.url()].clone();
        UnsyncFileMeta::new(file.filename, file.mime, source).extra(file.extra)
    };

    let contents = contents
        .into_iter()
        .map(|c| match c {
            PendingContent::Text(text) => UnsyncContent::Text(text),
            PendingContent::File(file) => UnsyncContent::File(into_meta(file)),
        })
        .collect();

    Ok((contents, thumb.map(into_meta)))
}

mod common {
    use html2md::TagHandler;

    use super::*;

    pub fn parse_description(artwork: &PixivArtwork) -> Vec<PendingContent> {
        let description = artwork.description.trim();
        if description.is_empty() {
            return vec![];
        }

        struct AnchorHandlerFactory;
        impl html2md::TagHandlerFactory for AnchorHandlerFactory {
            fn instantiate(&self) -> Box<dyn html2md::TagHandler> {
                Box::new(AnchorHandler(html2md::anchors::AnchorHandler::default()))
            }
        }

        struct AnchorHandler(html2md::anchors::AnchorHandler);
        impl TagHandler for AnchorHandler {
            fn handle(&mut self, tag: &html2md::Handle, printer: &mut html2md::StructuredPrinter) {
                if let html2md::NodeData::Element { attrs, .. } = &tag.data
                    && let mut attrs = attrs.borrow_mut()
                    && let Some(href) = attrs
                        .iter_mut()
                        .find(|attr| attr.name.local.to_string() == "href")
                        .filter(|attr| attr.value.to_string().starts_with("/jump.php?"))
                {
                    const PREFIX: usize = "/jump.php?".len();
                    let url = &href.value[PREFIX..];
                    let decoded = percent_encoding::percent_decode_str(url)
                        .decode_utf8()
                        .unwrap();

                    href.value = decoded.to_string().into();
                };
                self.0.handle(tag, printer);
            }

            fn after_handle(&mut self, printer: &mut html2md::StructuredPrinter) {
                self.0.after_handle(printer);
            }
        }

        let mut markdown = html2md::parse_html_custom(
            description,
            &HashMap::from([(
                "a".to_string(),
                Box::new(AnchorHandlerFactory) as Box<dyn html2md::TagHandlerFactory>,
            )]),
        );

        if matches!(&artwork.content, PixivArtworkContent::Novel { .. }) {
            markdown = markdown
                .lines()
                .map(|line| format!("> {line}"))
                .collect::<Vec<_>>()
                .join("\n");
        }

        vec![PendingContent::Text(markdown)]
    }

    pub fn parse_date(date: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(date).unwrap().to_utc()
    }

    pub fn get_author(artwork: &PixivArtwork) -> UnsyncAuthor {
        UnsyncAuthor::new(artwork.user_name.clone()).aliases(vec![
            UnsyncAlias::new("pixiv".to_string(), artwork.user_id.clone())
                .link(format!("https://www.pixiv.net/users/{}", artwork.user_id)),
        ])
    }

    pub async fn get_comments(client: &PixivClient, artwork: &PixivArtwork) -> Vec<Comment> {
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
            .map(|nav| {
                let is_novel = matches!(artwork.content, PixivArtworkContent::Novel { .. });
                nav.into_collection(&artwork.user_id, is_novel)
            })
            .into_iter()
            .collect()
    }

    pub async fn get_contents_and_thumb(
        client: &PixivClient,
        artwork: &PixivArtwork,
    ) -> Result<(Vec<PendingContent>, Option<PendingFile>)> {
        let mut contents = common::parse_description(artwork);
        let thumb: Option<PendingFile>;

        match &artwork.content {
            PixivArtworkContent::Illust { illust_type, .. } => {
                let file_metas = illust::fetch_pages(client, &artwork.id).await?;
                thumb = file_metas.first().cloned();

                match illust_type {
                    IllustType::Illust | IllustType::Manga => {
                        contents.extend(file_metas.into_iter().map(PendingContent::File));
                    }
                    IllustType::Ugoira => {
                        let extra = thumb.as_ref().map(|t| t.extra.clone()).unwrap_or_default();
                        let ugoira = client
                            .fetch::<PixivUgoira>(&format!(
                                "https://www.pixiv.net/ajax/illust/{}/ugoira_meta",
                                artwork.id
                            ))
                            .await?;

                        contents.push(PendingContent::File(PendingFile {
                            filename: "ugoira.webm".to_string(),
                            mime: "video/webm".to_string(),
                            extra,
                            request: ArchiveRequest::Ugoira {
                                url: ugoira.original_src,
                                frames: ugoira.frames,
                            },
                        }));
                    }
                }
            }
            PixivArtworkContent::Novel {
                content, cover_url, ..
            } => {
                contents.push(PendingContent::Text(content.clone()));
                thumb = Some(novel::parse_cover(cover_url));
            }
        };

        Ok((contents, thumb))
    }
}

mod illust {
    use post_archiver_utils::Result;

    use super::*;

    pub async fn fetch_pages(client: &PixivClient, artwork_id: &str) -> Result<Vec<PendingFile>> {
        let pages = client
            .fetch::<Vec<PixivIllustPages>>(&format!(
                "https://www.pixiv.net/ajax/illust/{}/pages?lang=ja",
                artwork_id
            ))
            .await?;

        Ok(pages
            .into_iter()
            .map(|page| {
                url_into_pending_file(page.urls.original, None, None).extra(HashMap::from([
                    ("width".to_string(), json!(page.width)),
                    ("height".to_string(), json!(page.height)),
                ]))
            })
            .collect())
    }
}

mod novel {
    use super::*;

    pub fn parse_cover(url: &str) -> PendingFile {
        url_into_pending_file(
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

fn url_into_pending_file(
    url: String,
    filename: Option<String>,
    size: Option<(u32, u32)>,
) -> PendingFile {
    let filename = filename.unwrap_or_else(|| {
        Url::parse(&url)
            .unwrap()
            .path_segments()
            .and_then(|mut segments| segments.next_back())
            .map(|s| s.to_string())
            .unwrap()
    });

    let mime = mime_guess::from_path(&filename).first_or_octet_stream();

    PendingFile {
        filename,
        mime: mime.to_string(),
        extra: HashMap::new(),
        request: match size {
            Some((width, height)) => ArchiveRequest::ImageWithSize { url, width, height },
            None => ArchiveRequest::Image(url),
        },
    }
}
