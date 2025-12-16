use std::sync::Arc;

use fast_image_resize::{ResizeOptions, Resizer};
use futures::future::try_join_all;
use image::{DynamicImage, ImageReader};
use log::{error, warn};
use plyne::Output;
use post_archiver_utils::Result;
use serde::Deserialize;
use tempfile::TempPath;
use tokio::{sync::Semaphore, task::JoinSet};

use crate::{
    FileEvent,
    api::PixivClient,
    config::{Config, Progress},
};

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

pub async fn download_files(mut files_pipeline: Output<FileEvent>, config: &Config) {
    let files_pb = Progress::new(config.multi.clone(), "files");

    let mut tasks = JoinSet::new();
    let client = PixivClient::new(config);
    let semaphore = Arc::new(Semaphore::new(3));
    while let Some((reqs, tx)) = files_pipeline.recv().await {
        if reqs.is_empty() {
            tx.send(Default::default()).unwrap();
            continue;
        }

        let semaphore = semaphore.clone();
        let files_pb = files_pb.clone();
        let client = client.clone();
        files_pb.inc_length(reqs.len() as u64);
        tasks.spawn(async move {
            let _permit = semaphore.acquire().await.unwrap();
            match try_join_all(reqs.into_iter().map(async |req| {
                let url = req.url().to_string();
                let result = download_file(req, &client).await.map(|dst| (url, dst));
                files_pb.inc(1);
                result
            }))
            .await
            {
                Ok(results) => tx.send(results.into_iter().collect()).unwrap(),
                Err(e) => error!("Failed to download files: {e}"),
            }
        });
    }

    tasks.join_all().await;
    files_pb.finish();
}

async fn download_file(request: ArchiveRequest, client: &PixivClient) -> Result<TempPath> {
    let dst = client.as_inner().download(request.url()).await?;

    match request {
        ArchiveRequest::Image(_) => Ok(dst),
        ArchiveRequest::ImageWithSize {
            url: _,
            width,
            height,
        } => {
            // TODO: move resizer to a separate thread
            resize(dst, width, height)
        }
        ArchiveRequest::Ugoira { url, frames: _ } => {
            error!("Ugoira download not implemented yet: {url}");
            Err("Ugoira download not implemented yet")
        }
    }
    .map_err(|e: &'static str| {
        error!("Failed to process file: {e}");
        post_archiver_utils::Error::InvalidResponse(e.to_string())
    })
}

fn resize(path: TempPath, width: u32, height: u32) -> std::result::Result<TempPath, &'static str> {
    let src_image = ImageReader::open(&path)
        .map_err(|e| {
            warn!("Failed to open image: {e}");
            "Failed to open image"
        })?
        .decode()
        .map_err(|e| {
            warn!("Failed to decode image: {e}");
            "Failed to decode image"
        })?;

    if src_image.width() != width || src_image.height() != height {
        let mut dst_image = DynamicImage::new(width, height, src_image.color());

        let mut resizer = Resizer::new();
        resizer
            .resize(
                &src_image,
                &mut dst_image,
                &Some(ResizeOptions::new().fit_into_destination(None)),
            )
            .map_err(|e| {
                warn!("Failed to resize image: {e}");
                "Failed to resize image"
            })?;

        dst_image.save(&path).map_err(|e| {
            warn!("Failed to save resized image: {e}");
            "Failed to save resized image"
        })?;
    }
    Ok(path)
}
