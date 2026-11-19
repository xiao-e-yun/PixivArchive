use std::sync::Arc;

use api::PixivClient;
use artwork::{PixivArtworkId, artworks};
use config::{Config, ProgressSet};
use favorite::reslove_current_user;
use log::{info, warn};
use plyne::{Input, define_tasks};
use post_archiver::{PlatformId, manager::PostArchiverManager};
use post_archiver_utils::{display_metadata, setup_database};
use series::{PixivSeriesId, reslove_series};
use tokio::sync::Mutex;
use user::{PixivUserId, reslove_users};

pub mod api;
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
    display_metadata!("Pixiv Archive" {
        "Version": format!("v{}", env!("CARGO_PKG_VERSION")),
        "PostArchiver Version": post_archiver::utils::VERSION,
        "Strategy": config.strategy,
        "Output": config.output.display(),
        "Limit": format!("{}/min", config.limit),
        "Users": config.users.len(),
        "Illusts": config.illusts.len(),
        "Novels": config.novels.len(),
        "Illust Series": config.illust_series.len(),
        "Novel Series": config.novel_series.len(),
        "Followed Users": yes_or_no(config.followed_users),
        "Favorite": yes_or_no(config.favorite),
    });

    if config.users.is_empty()
        && config.illusts.is_empty()
        && config.novels.is_empty()
        && config.illust_series.is_empty()
        && config.novel_series.is_empty()
        && !config.followed_users
        && !config.favorite
    {
        warn!("[main] No targets specified.");
        warn!("[main] Run with --help for more information.");
        return;
    }

    if !config.has_ffmpeg {
        warn!("[main] ffmpeg not found — Ugoira artworks will be skipped");
    }

    let mut manager = match setup_database(&config.output) {
        Ok(manager) => manager,
        Err(e) => {
            log::error!("[main] Failed to open archive: {e}");
            return;
        }
    };
    // Tags are platform-scoped, so resolve the platform once up front.
    let platform = manager
        .importer()
        .import_platform("pixiv".to_string())
        .expect("Failed to import platform");

    let client = PixivClient::new(&config);
    let progress = ProgressSet::new(&config.multi);

    PixivSystem::new(
        Arc::new(Mutex::new(manager)),
        config,
        client,
        platform,
        progress,
    )
    .execute()
    .await;

    info!("[main] Archive completed");
}

pub type Manager = Arc<Mutex<PostArchiverManager>>;

define_tasks! {
    PixivSystem
    pipelines {
        users_pipeline: PixivUserId,
        series_pipeline: PixivSeriesId,
        artworks_pipeline: PixivArtworkId,
    }
    vars {
        manager: Manager,
        config: Config,
        client: PixivClient,
        platform: PlatformId,
        progress: ProgressSet,
    }
    tasks {
        resolve_main,
        reslove_current_user,
        reslove_users,
        reslove_series,
        artworks,
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

