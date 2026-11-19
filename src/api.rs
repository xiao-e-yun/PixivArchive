use std::{
    num::NonZeroU32,
    path::{Path, PathBuf},
};

use indicatif::ProgressBar;
use log::debug;
use post_archiver_utils::{ArchiveClient, Limit, RequestError, Result, browser_headers};
use reqwest::{
    Client,
    header::{self, HeaderMap, HeaderValue},
};
use serde::{Deserialize, de::DeserializeOwned};

use crate::config::{Config, IncOnDrop};

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
            NullableBody::None(_) => Err(RequestError::InvalidResponse(self.message).into()),
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
            Err(RequestError::InvalidResponse(self.message).into())
        } else {
            Ok(self.body)
        }
    }
}

/// Headers shared by API and image requests. Without the Pixiv Referer, i.pximg.net returns 403.
fn pixiv_headers(config: &Config) -> HeaderMap {
    let mut headers = browser_headers(&config.user_agent);
    headers.insert(header::REFERER, HeaderValue::from_static("https://www.pixiv.net/"));
    headers.insert(
        header::COOKIE,
        format!("PHPSESSID={}", config.session).parse().unwrap(),
    );
    headers
}

#[derive(Debug, Clone)]
pub struct PixivClient {
    inner: ArchiveClient,
}

impl PixivClient {
    pub fn new(config: &Config) -> Self {
        let mut headers = pixiv_headers(config);
        // Origin must not have a trailing slash.
        headers.insert(header::ORIGIN, HeaderValue::from_static("https://www.pixiv.net"));

        let client = Client::builder().default_headers(headers).build().unwrap();
        let limit = Limit {
            per_min: config.limit,
            ..Default::default()
        };

        Self {
            inner: ArchiveClient::new(client, limit),
        }
    }

    pub async fn fetch<T: DeserializeOwned>(&self, url: &str) -> Result<T> {
        match self.inner.fetch::<PixivResponse<T>>(url).await {
            Ok(response) => response.downcast(),
            Err(RequestError::UnexpectedStatus(status, body)) => {
                debug!("{status} response body: {body}");
                Err(RequestError::UnexpectedStatus(status, body).into())
            }
            Err(e) => Err(e.into()),
        }
    }
}

/// Downloads images and ugoira zips from i.pximg.net with its own rate limit,
/// so file downloads don't eat into the API budget.
#[derive(Debug, Clone)]
pub struct PixivDownloader {
    inner: ArchiveClient,
    pb: ProgressBar,
}

impl PixivDownloader {
    pub fn new(config: &Config, pb: ProgressBar) -> Self {
        let client = Client::builder()
            .default_headers(pixiv_headers(config))
            .build()
            .unwrap();
        let limit = Limit {
            per_min: NonZeroU32::new(90).unwrap(),
            ..Default::default()
        };

        Self {
            inner: ArchiveClient::new(client, limit),
            pb,
        }
    }

    pub async fn download(&self, with_in: &Path, url: &str) -> Result<PathBuf> {
        self.pb.inc_length(1);
        let _guard = IncOnDrop(self.pb.clone());
        Ok(self.inner.download(with_in, url).await?)
    }
}
