use std::collections::HashMap;

use artwork::{PixivArtwork, PixivArtworkId, archive_artworks, resolve_artworks};
use config::Config;
use favorite::reslove_current_user;
use file::{ArchiveRequest, download_files};
use log::{info, warn};
use plyne::{Input, define_tasks};
use post_archiver::{
    Comment,
    importer::{UnsyncContent, UnsyncFileMeta},
    manager::PostArchiverManager,
};
use post_archiver_utils::{ArchiveClient, Error, Result, display_metadata};
use reqwest::{
    Client,
    header::{COOKIE, HeaderMap, HeaderValue, REFERER},
};
use serde::{Deserialize, de::DeserializeOwned};
use series::{PixivSeriesId, reslove_series};
use tempfile::TempPath;
use tokio::sync::Mutex;
use user::{PixivUserId, reslove_users};

pub mod artwork;
pub mod comment;
pub mod config;
pub mod favorite;
pub mod file;
pub mod series;
pub mod tag;
pub mod user;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let config = Config::init();

    fn yes_or_no(value: bool) -> &'static str {
        if value { "Yes" } else { "No" }
    }
    display_metadata(
        "Pixiv Archive",
        &[
            (
                "Version",
                format!("v{}", env!("CARGO_PKG_VERSION")).as_str(),
            ),
            ("Overwrite", yes_or_no(config.overwrite)),
            ("Output", config.output.to_str().unwrap()),
            ("Limit", &config.limit.to_string()),
            ("Users", &config.users.len().to_string()),
            ("Illusts", &config.illusts.len().to_string()),
            ("Novels", &config.novels.len().to_string()),
            ("Illust Series", &config.illust_series.len().to_string()),
            ("Novel Series", &config.novel_series.len().to_string()),
            ("Followed Users", yes_or_no(config.followed_users)),
            ("Favorite", yes_or_no(config.favorite)),
        ],
    );

    if !config.output.exists() {
        warn!("[main] Creating output folder");
        std::fs::create_dir_all(&config.output).unwrap();
    }

    info!("[main] Connecting to PostArchiver");
    let manager = PostArchiverManager::open_or_create(&config.output).unwrap();

    let client = client(&config);

    PixivSystem::new(Mutex::new(manager), config, client)
        .execute()
        .await;

    info!("[main] Archive completed");
}

pub type Manager = Mutex<PostArchiverManager>;

pub type FileEvent = (
    Vec<ArchiveRequest>,
    tokio::sync::oneshot::Sender<HashMap<String, TempPath>>,
);

#[derive(Debug)]
pub struct SyncEvent {
    source: String,
    artwork: PixivArtwork,
    contents: Vec<UnsyncContent<ArchiveRequest>>,
    thumb: Option<UnsyncFileMeta<ArchiveRequest>>,
    comments: Vec<Comment>,
    files: tokio::sync::oneshot::Receiver<HashMap<String, TempPath>>,
}

define_tasks! {
    PixivSystem
    pipelines {
        users_pipeline: PixivUserId,
        series_pipeline: PixivSeriesId,
        artworks_pipeline: PixivArtworkId,
        files_pipeline: FileEvent,
        sync_pipeline: SyncEvent,
    }
    vars {
        manager: Manager,
        config: Config,
        client: ArchiveClient,
    }
    tasks {
        resolve_main,
        reslove_current_user,
        reslove_users,
        reslove_series,
        resolve_artworks,
        archive_artworks,
        download_files,
    }
}

async fn resolve_main(
    users_pipeline: Input<PixivUserId>,
    series_pipeline: Input<PixivSeriesId>,
    artworks_pipeline: Input<PixivArtworkId>,
    config: &Config,
) {
    for user in &config.users {
        info!("[main] Archive user: {user:?}");
        users_pipeline.send(*user).unwrap();
    }

    macro_rules! remap {
        ($series: expr, $fn: expr) => {
            $series.iter().cloned().map($fn)
        };
    }

    for illust_series in remap!(config.illust_series, PixivSeriesId::Illust) {
        info!("[main] Archive Illust Series: {illust_series:?}");
        series_pipeline.send(illust_series).unwrap();
    }
    for novel_series in remap!(config.novel_series, PixivSeriesId::Novel) {
        info!("[main] Archive Novel Series: {novel_series:?}");
        series_pipeline.send(novel_series).unwrap();
    }

    for illusts in remap!(config.illusts, PixivArtworkId::Illust) {
        info!("[main] Archive Illusts: {illusts:?}");
        artworks_pipeline.send(illusts).unwrap();
    }
    for novels in remap!(config.novels, PixivArtworkId::Novel) {
        info!("[main]   Novel Series: {novels:?}");
        artworks_pipeline.send(novels).unwrap();
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct PixivResponse<T> {
    pub error: bool,
    pub message: String,
    pub body: NullableBody<T>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum NullableBody<T> {
    Some(T),
    None([(); 0]),
}

impl<T> PixivResponse<T> {
    pub fn downcast(self) -> Result<T> {
        match self.body {
            NullableBody::Some(body) => Ok(body),
            NullableBody::None(_) => Err(Error::InvalidResponse(self.message)),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct PixivResponseUnwrap<T> {
    pub error: bool,
    pub message: String,
    pub body: T,
}

impl<T> PixivResponseUnwrap<T> {
    pub fn downcast(self) -> Result<T> {
        if self.error {
            Err(Error::InvalidResponse(self.message))
        } else {
            Ok(self.body)
        }
    }
}

pub fn client(config: &Config) -> ArchiveClient {
    ArchiveClient::builder(
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
    )
    .build()
}
pub async fn fetch<T: DeserializeOwned>(client: &ArchiveClient, url: &str) -> Result<T> {
    client
        .fetch::<PixivResponse<T>>(url)
        .await
        .and_then(|r| r.downcast())
}
