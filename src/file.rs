use std::{collections::HashSet, fs::File, path::PathBuf, sync::Arc};

use fast_image_resize::{ResizeOptions, Resizer};
use image::{DynamicImage, ImageReader};
use log::{debug, error, info};
use post_archiver_utils::ArchiveClient;
use reqwest::{
    Client,
    header::{COOKIE, HeaderMap, HeaderValue, REFERER},
};
use serde::Deserialize;
use tokio::{
    fs,
    sync::{Semaphore, mpsc::UnboundedReceiver},
    task::JoinSet,
};

use crate::config::{Config, ProgressManager};

#[derive(Debug, Clone, Deserialize)]
pub enum ArchiveRequest {
    Image(String),
    ImageWithSize {
        url: String,
        width: u32,
        height: u32,
    },
    Ugoira {
        url: String,
        frames: Vec<PixivUgoiraFrame>,
    },
}

impl ArchiveRequest {
    pub fn url(&self) -> &str {
        match self {
            ArchiveRequest::Image(url) => url,
            ArchiveRequest::ImageWithSize { url, .. } => url,
            ArchiveRequest::Ugoira { url, .. } => url,
        }
    }

    pub fn size(&self) -> Option<(u32, u32)> {
        match self {
            ArchiveRequest::ImageWithSize { width, height, .. } => Some((*width, *height)),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct PixivUgoira {
    pub original_src: String,
    pub src: String,
    pub mime_type: String,
    pub frames: Vec<PixivUgoiraFrame>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PixivUgoiraFrame {
    pub delay: u32,
    pub file: String,
}

pub async fn archive_files(config: &Config, mut rx: UnboundedReceiver<(PathBuf, ArchiveRequest)>) {
    let client = ArchiveClient::new(
        Client::builder()
            .default_headers(HeaderMap::from_iter([
                (
                    COOKIE,
                    HeaderValue::from_str(&format!("PHPSESSID={}", config.session)).unwrap(),
                ),
                (
                    REFERER,
                    HeaderValue::from_str("https://www.pixiv.net/").unwrap(),
                ),
            ]))
            .user_agent(&config.user_agent)
            .build()
            .unwrap(),
        config.limit,
    );

    let mut join_set = JoinSet::new();
    let pb = ProgressManager::new(config.multi.clone(), "file");

    let mut created_folders = HashSet::new();

    debug!("[file] Waiting for file to download");
    let semaphore = Arc::new(Semaphore::new(3));
    while let Some((path, request)) = rx.recv().await {
        let pb = pb.clone();
        pb.inc_length(1);

        if !config.overwrite && path.exists() {
            info!("[file] File already exists, skipping: {}", path.display());
            pb.inc(1);
            continue;
        }

        let folder = path.parent().unwrap();
        if !created_folders.contains(folder) && !folder.exists() {
            debug!("[file] Creating post folder: {}", folder.display());
            created_folders.insert(folder.to_path_buf());
            if let Err(e) = fs::create_dir_all(folder).await {
                error!("[file] Failed to create post folder: {e:?}");
            };
        };

        let client = client.clone();
        let semaphore = semaphore.clone();
        join_set.spawn(async move {
            let _ = semaphore.acquire().await.unwrap();
            info!("[file] Download: {}", path.display());

            if archive_file(client, path.clone(), request).await.is_none() {
                error!("[file] Failed to download {}", path.display());
                return;
            };

            info!("[file] Downloaded {}", path.display());
            pb.inc(1);
        });
    }

    if join_set.is_empty() {
        return;
    }
    join_set.join_all().await;

    info!("[file] Download finished");
}

pub async fn archive_file(
    client: ArchiveClient,
    path: PathBuf,
    request: ArchiveRequest,
) -> Option<()> {
    let mut file = match &request {
        ArchiveRequest::Image(_) | ArchiveRequest::ImageWithSize { .. } => File::create(&path),
        ArchiveRequest::Ugoira { .. } => tempfile::tempfile(),
    }
    .ok()?;

    debug!("[file] Downloading: {}", request.url());
    if let Err(e) = client.download(request.url(), &mut file).await {
        error!("[file] Failed to download {}: {}", request.url(), e);
        return None;
    }

    // TODO: move resizer to a separate thread
    if let Some((width, height)) = request.size() {
        let src_image = ImageReader::open(&path).unwrap().decode().unwrap();
        if src_image.width() != width || src_image.height() != height {
            let mut dst_image = DynamicImage::new(width, height, src_image.color());

            let mut resizer = Resizer::new();
            resizer
                .resize(
                    &src_image,
                    &mut dst_image,
                    &Some(ResizeOptions::new().fit_into_destination(None)),
                )
                .unwrap();

            dst_image.save(path).ok()?;
        }
    };

    if let ArchiveRequest::Ugoira {  .. } = request {
        todo!("Handle Ugoira frames");
    }

    Some(())
}
