use std::path::PathBuf;

use artwork::{PixivArtworkId, resolve_artworks};
use config::Config;
use file::{ArchiveRequest, archive_files};
use log::{debug, info, warn};
use post_archiver::manager::PostArchiverManager;
use post_archiver_utils::{ArchiveClient, Error, Result, display_metadata};
use reqwest::{
    header::{HeaderMap, HeaderValue, COOKIE, REFERER}, Client
};
use serde::{Deserialize, de::DeserializeOwned};
use series::{PixivSeriesId, reslove_series};
use tokio::{
    join,
    sync::mpsc::{UnboundedSender, unbounded_channel},
};
use user::{PixivUserId, reslove_users};

pub mod artwork;
pub mod comment;
pub mod config;
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
            ("Version", format!("v{}", env!("CARGO_PKG_VERSION")).as_str()),
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

    let client = ArchiveClient::new(
        Client::builder()
            .default_headers(HeaderMap::from_iter([
                (COOKIE, HeaderValue::from_str(&format!("PHPSESSID={}", config.session)).unwrap()),
                (REFERER, HeaderValue::from_str("https://www.pixiv.net/").unwrap())
            ]))
            .user_agent(&config.user_agent)
            .build()
            .unwrap(),
        config.limit,
    );

    let (tx_users, rx_users) = unbounded_channel::<PixivUserId>();
    let (tx_artworks, rx_authors) = unbounded_channel::<PixivArtworkId>();
    let (tx_series, rx_series) = unbounded_channel::<PixivSeriesId>();
    let (tx_files, rx_files) = unbounded_channel::<(PathBuf, ArchiveRequest)>();

    reslove_all(&config, tx_users, tx_series, tx_artworks.clone());

    join!(
        reslove_users(&config, client.clone(), rx_users, tx_artworks.clone()),
        reslove_series(&config, &client, rx_series, tx_artworks),
        resolve_artworks(&config, manager, &client, rx_authors, tx_files),
        archive_files(&config, rx_files),
    );

    info!("[main] Archive completed");
}

fn reslove_all(
    config: &Config,
    tx_users: UnboundedSender<PixivUserId>,
    tx_series: UnboundedSender<PixivSeriesId>,
    tx_artworks: UnboundedSender<PixivArtworkId>,
) {
    for user in &config.users {
        info!("[main] Archive user: {user}");
        tx_users.send(*user).unwrap();
    }

    for post in config
        .illusts
        .iter()
        .cloned()
        .map(PixivArtworkId::Illust)
        .chain(config.novels.iter().cloned().map(PixivArtworkId::Novel))
    {
        info!("[main] Archive artwork: {post:?}");
        tx_artworks.send(post).unwrap();
    }

    for series in config
        .illust_series
        .iter()
        .cloned()
        .map(PixivSeriesId::Illust)
        .chain(
            config
                .novel_series
                .iter()
                .cloned()
                .map(PixivSeriesId::Novel),
        )
    {
        info!("[main] Archive series: {series:?}");
        tx_series.send(series).unwrap();
    }

    // TODO: followed_users
    // TODO: favorite
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

pub async fn fetch<T: DeserializeOwned>(client: &ArchiveClient, url: &str) -> Result<T> {
    debug!("[fetch] Fetching: {url}");
    client
        .fetch::<PixivResponse<T>>(url)
        .await
        .and_then(|r| r.downcast())
}
