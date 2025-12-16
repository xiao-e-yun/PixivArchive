use chrono::Utc;
use clap::{Parser, ValueEnum};
use clap_verbosity_flag::{InfoLevel, Verbosity};
use dotenv::dotenv;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use indicatif_log_bridge::LogWrapper;
use std::{ops::Deref, path::PathBuf};

use crate::PixivUserId;

#[derive(Debug, Clone, Parser, Default)]
pub struct Config {
    /// Your `PHPSESSID` cookie
    #[clap(env = "PHPSESSID")]
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
    /// Which you path want to save
    #[arg(default_value = "./archive", env = "OUTPUT")]
    pub output: PathBuf,
    /// Overwrite existing files
    #[arg(short, long)]
    pub overwrite: bool,
    #[arg(short, long, default_value = "")]
    pub user_agent: String,
    /// Limit the number of concurrent copys
    #[arg(short, long, default_value = "30")]
    pub limit: u32,
    #[command(flatten)]
    pub verbose: Verbosity<InfoLevel>,
    #[clap(skip)]
    pub multi: MultiProgress,
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

#[derive(Debug, Clone)]
pub struct Progress(ProgressBar);

impl Progress {
    pub fn new(multi: MultiProgress, prefix: &'static str) -> Self {
        Self(
            multi.add(
                ProgressBar::new(0)
                    .with_style(Self::style())
                    .with_prefix(format!("[{prefix}]")),
            ),
        )
    }

    fn style() -> ProgressStyle {
        ProgressStyle::with_template("{prefix:.bold.dim} {wide_bar:.cyan/blue} {pos:>3}/{len:3}")
            .unwrap()
            .progress_chars("#>-")
    }
}

impl Deref for Progress {
    type Target = ProgressBar;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
