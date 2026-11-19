use std::{
    fmt::Write,
    path::{Path, PathBuf},
};

use fast_image_resize::{ResizeOptions, Resizer};
use image::{DynamicImage, ImageReader};
use log::{error, warn};
use post_archiver_utils::{RequestError, Result};
use serde::Deserialize;

use crate::api::PixivDownloader;

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

/// Download a file into `with_in` and post-process it there (resize / ugoira conversion).
pub async fn download_file(
    request: ArchiveRequest,
    downloader: &PixivDownloader,
    with_in: &Path,
) -> Result<PathBuf> {
    let dst = downloader.download(with_in, request.url()).await?;

    let processed = match request {
        ArchiveRequest::Image(_) => Ok(dst),
        ArchiveRequest::ImageWithSize { width, height, .. } => {
            // Decoding and resizing is CPU-bound; keep it off the current_thread runtime.
            tokio::task::spawn_blocking(move || resize(dst, width, height))
                .await
                .unwrap_or(Err("Resize task panicked"))
        }
        ArchiveRequest::Ugoira { frames, .. } => convert_ugoira(dst, frames, with_in).await,
    };

    processed.map_err(|e| {
        error!("Failed to process file: {e}");
        RequestError::InvalidResponse(e.to_string()).into()
    })
}

async fn convert_ugoira(
    zip_path: PathBuf,
    frames: Vec<PixivUgoiraFrame>,
    with_in: &Path,
) -> std::result::Result<PathBuf, &'static str> {
    let frames_dir =
        tempfile::tempdir_in(with_in).map_err(|_| "Failed to create temp dir for ugoira")?;
    let frames_path = frames_dir.path().to_path_buf();
    let concat_path = frames_path.join("concat.txt");

    let concat = concat_path.clone();
    let output_name = format!(
        "ugoira-{}.webm",
        frames_path.file_name().unwrap().to_string_lossy()
    );
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
            let outpath = frames_path.join(&name);

            let mut outfile =
                std::fs::File::create(&outpath).map_err(|_| "Failed to create frame file")?;
            std::io::copy(&mut entry, &mut outfile).map_err(|_| "Failed to extract frame")?;
        }
        // The zip is no longer needed once the frames are extracted.
        let _ = std::fs::remove_file(&zip_path);

        let last_frame = frames.last().ok_or("Ugoira has no frames")?;
        let mut content = String::new();
        for frame in &frames {
            let frame_path = frames_path.join(&frame.file);
            writeln!(content, "file '{}'", frame_path.display()).unwrap();
            writeln!(content, "duration {}", frame.delay as f64 / 1000.0).unwrap();
        }
        // ffmpeg's concat demuxer ignores the last duration unless the last frame is repeated.
        writeln!(content, "file '{}'", frames_path.join(&last_frame.file).display()).unwrap();

        std::fs::write(concat, content).map_err(|_| "Failed to write concat file")
    })
    .await
    .map_err(|_| "Blocking task panicked")??;

    let output_path = with_in.join(output_name);

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

    Ok(output_path)
}

fn resize(path: PathBuf, width: u32, height: u32) -> std::result::Result<PathBuf, &'static str> {
    let src_image = ImageReader::open(&path)
        .map_err(|e| {
            warn!("Failed to open image: {e}");
            "Failed to open image"
        })?
        .with_guessed_format()
        .map_err(|e| {
            warn!("Failed to detect image format: {e}");
            "Failed to detect image format"
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

        // Downloaded files have random names without an extension, so name the format explicitly.
        // JPEG has no alpha channel, so drop it before encoding.
        DynamicImage::ImageRgb8(dst_image.to_rgb8())
            .save_with_format(&path, image::ImageFormat::Jpeg)
            .map_err(|e| {
                warn!("Failed to save resized image: {e}");
                "Failed to save resized image"
            })?;
    }
    Ok(path)
}
