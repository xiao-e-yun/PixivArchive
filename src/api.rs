use post_archiver_utils::{ArchiveClient, Error, Result};
use reqwest::{
    Client,
    header::{HeaderMap, HeaderValue, COOKIE, REFERER},
};
use serde::{de::DeserializeOwned, Deserialize};

use crate::config::Config;

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

#[derive(Debug, Clone)]
pub struct PixivClient {
    inner: ArchiveClient,
}

impl PixivClient {
    pub fn new(config: &Config) -> Self {
        let inner = ArchiveClient::builder(
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
        .build();

        Self { inner }
    }

    pub async fn fetch<T: DeserializeOwned>(&self, url: &str) -> Result<T> {
        self.inner
            .fetch::<PixivResponse<T>>(url)
            .await
            .and_then(|r| r.downcast())
    }

    pub fn as_inner(&self) -> &ArchiveClient {
        &self.inner
    }
}
