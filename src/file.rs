use std::{sync::Arc};

use fast_image_resize::{ResizeOptions, Resizer};
use futures::future::try_join_all;
use image::{DynamicImage, ImageReader};
use log::{error, warn};
use plyne::Output;
use post_archiver_utils::Result;
use serde::Deserialize;
use tempfile::TempPath;
use tokio::{sync::Semaphore, task::JoinSet};
use std::fmt::Write;

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
    pub src: String,
    #[serde(rename = "originalSrc")]
    pub original_src: String,
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
        ArchiveRequest::Ugoira { url: _, frames } => convert_ugoira(dst, frames).await,
    }
    .map_err(|e: &'static str| {
        error!("Failed to process file: {e}");
        post_archiver_utils::Error::InvalidResponse(e.to_string())
    })
}

async fn convert_ugoira(
    zip_path: TempPath,
    frames: Vec<PixivUgoiraFrame>,
) -> std::result::Result<TempPath, &'static str> {
    let temp_dir = tempfile::tempdir().map_err(|_| "Failed to create temp dir for ugoira")?;
    let temp_dir_path = temp_dir.path().to_path_buf();

    let concat_path = temp_dir_path.join("concat.txt");
    let concat_path_cloned = concat_path.clone();
    tokio::task::spawn_blocking(move || -> std::result::Result<(), &'static str> {
        let zip_file = std::fs::File::open(&zip_path).map_err(|_| "Failed to open ugoira zip")?;
        let mut archive =
            zip::ZipArchive::new(zip_file).map_err(|_| "Failed to parse ugoira zip")?;

        for i in 0..archive.len() {
            let mut entry = archive
                .by_index(i)
                .map_err(|_| "Failed to read zip entry")?;

            // 000003.jpg
            let name = entry
                .enclosed_name()
                .ok_or("Unsafe zip entry name")?
                .to_path_buf();
            let outpath = temp_dir_path.join(&name);

            let mut outfile =
                std::fs::File::create(&outpath).map_err(|_| "Failed to create frame file")?;
            std::io::copy(&mut entry, &mut outfile).map_err(|_| "Failed to extract frame")?;
        }

        let last_frame = frames.last().ok_or("Ugoira has no frames")?;
        let mut content = String::new();
        for frame in &frames {
            let is_last = frame.file == last_frame.file;
            let frame_path = temp_dir_path.join(&frame.file);
            writeln!(content, "file '{}'",frame_path.display()).unwrap();
            writeln!(content, "duration {}", frame.delay as f64 / 1000.0).unwrap();
            
            if is_last { 
                writeln!(content, "file '{}'",frame_path.display()).unwrap();
            }
        };

        use std::io::Write;
        let mut file = std::fs::File::create(concat_path_cloned).map_err(|_| "Failed to create concat file")?;
        file.write(content.as_bytes()).map_err(|_| "Failed to write concat file")?;
        file.flush().map_err(|_| "Failed to flush concat file")?;

        Ok(())
    })
    .await
    .map_err(|_| "Blocking task panicked")?
    .map_err(|e| e)?;

    let output = tempfile::NamedTempFile::new().map_err(|_| "Failed to create output temp file")?;
    let output_path = output.path().to_path_buf();

    warn!("{}", concat_path.display());
    warn!("{}", std::fs::read_to_string(&concat_path).unwrap_or_else(|_| "Failed to read concat file".into()));
    let result = tokio::process::Command::new("ffmpeg")
        .args([
            "-y",
            "-f",
            "concat",
            "-safe",
            "0",
            "-i",
            concat_path.to_str().ok_or("Invalid concat path")?,
            "-c:v",
            "libvpx-vp9",
            "-b:v",
            "0",
            "-pix_fmt",
            "yuv420p",
            "-loglevel",
            "error",
            "-f",
            "webm",
            output_path.to_str().ok_or("Invalid output path")?,
        ])
        .output()
        .await
        .map_err(|_| "Failed to spawn ffmpeg (is ffmpeg installed?)")?;

    if !result.status.success() {
        let stderr = String::from_utf8_lossy(&result.stderr);
        error!("[ugoira] ffmpeg failed: {stderr}");
        return Err("ffmpeg conversion failed");
    }

    Ok(output.into_temp_path())
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
