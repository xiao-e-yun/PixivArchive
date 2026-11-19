use chrono::Utc;
use clap::{Parser, ValueEnum};
use clap_verbosity_flag::{InfoLevel, Verbosity};
use dotenv::dotenv;
use indicatif::{MultiProgress, ProgressBar};
use indicatif_log_bridge::LogWrapper;
use post_archiver_utils::{Strategy, define_progress};
use std::{num::NonZeroU32, path::PathBuf};

use crate::PixivUserId;

#[derive(Debug, Clone, Parser)]
pub struct Config {
    /// Your `PHPSESSID` cookie
    #[clap(env = "PHPSESSID", hide_env_values = true)]
    pub session: String,

    /// archive Id of Users
    #[arg(long, num_args = 0..)]
    pub users: Vec<PixivUserId>,

    /// archive Id of Illusts
    #[arg(long, num_args = 0..)]
    pub illusts: Vec<u64>,

    /// archive Id of Novels
    #[arg(long, num_args = 0..)]
    pub novels: Vec<u64>,

    /// archive Id of Illust Series
    #[arg(long, num_args = 0..)]
    pub illust_series: Vec<u64>,

    /// archive Id of Novel Series
    #[arg(long, num_args = 0..)]
    pub novel_series: Vec<u64>,

    /// archive followed users
    #[arg(long)]
    pub followed_users: bool,

    /// archive favorite artworks
    #[arg(long)]
    pub favorite: bool,

    // /// archive user categories
    // #[arg(short, long, num_args = 0..)]
    // pub categories: Vec<ArchiveCategory>,
    /// Archive directory to save into
    #[arg(short, long, default_value = "./archive", env = "OUTPUT")]
    pub output: PathBuf,
    /// Archiving strategy (`full` re-archives existing posts)
    #[arg(long, default_value_t)]
    pub strategy: Strategy,
    #[arg(short, long, default_value = "")]
    pub user_agent: String,
    /// Limit API requests per minute
    #[arg(short, long, default_value = "60")]
    pub limit: NonZeroU32,
    #[command(flatten)]
    pub verbose: Verbosity<InfoLevel>,
    #[clap(skip)]
    pub multi: MultiProgress,
    #[clap(skip)]
    pub has_ffmpeg: bool,
}

impl Config {
    pub fn init() -> Self {
        dotenv().ok();
        let mut config = <Self as Parser>::parse();

        let level = config.verbose.log_level_filter();
        let logger = env_logger::Builder::new()
            .filter_level(level)
            .format_target(false)
            .build();

        LogWrapper::new(config.multi.clone(), logger)
            .try_init()
            .unwrap();

        if config.user_agent.is_empty() {
            let dt = Utc::now().timestamp_millis() as u64 / 1000;
            let major = dt % 2 + 4;
            let webkit = dt / 2 % 64;
            let chrome = dt / 128 % 5 + 132;
            config.user_agent = format!(
                "Mozilla/{major}.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.{webkit} (KHTML, like Gecko) Chrome/{chrome}.0.0.0 Safari/537.{webkit}"
            );
        }

        config.has_ffmpeg = std::process::Command::new("ffmpeg")
            .arg("-version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success());

        log::set_max_level(level);
        config
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ArchiveCategory {
    Illusts,
    Manga,
    Novels,
    MangaSeries,
    NovelSeries,
}

define_progress!(ProgressSet [
    users,
    series,
    artworks,
    files
]);

/// Increments the progress bar by one when dropped, so early returns are still counted.
pub struct IncOnDrop(pub ProgressBar);

impl Drop for IncOnDrop {
    fn drop(&mut self) {
        self.0.inc(1);
    }
}
