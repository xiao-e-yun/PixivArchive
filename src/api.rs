use log::debug;
use post_archiver_utils::{ArchiveClient, Error, Result};
use reqwest::{
    Client,
    header::{self, HeaderMap},
};
use serde::{Deserialize, de::DeserializeOwned};

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
        let mut default_headers = Self::generate_user_headers(&config.user_agent);
        debug!("Using headers: {default_headers:#?} (without cookies)");

        default_headers.insert(header::COOKIE, format!("PHPSESSID={}", config.session).parse().unwrap());

        let inner = ArchiveClient::builder(
            Client::builder()
                .default_headers(default_headers)
                .build()
                .unwrap(),
            config.limit,
        )
        .pre_sec_limit((config.limit as f32 / 60.0).ceil() as u32)
        .build();

        Self { inner }
    }

    pub fn generate_user_headers(user_agent: &str) -> HeaderMap {
        let platform = if user_agent.contains("Windows") || user_agent.contains("Win64") {
            "\"Windows\""
        } else if user_agent.contains("Macintosh") || user_agent.contains("Mac OS X") {
            "\"macOS\""
        } else if user_agent.contains("Linux") || user_agent.contains("X11") {
            "\"Linux\""
        } else {
            "\"Unknown\""
        };

        let mobile = if user_agent.contains("Mobile") {
            "?1"
        } else {
            "?0"
        };

        let ua = if user_agent.contains("Edg/") {
            "Edg"
        } else if user_agent.contains("Chrome/") {
            "Chromium"
        } else if user_agent.contains("Firefox/") {
            "Firefox"
        } else if user_agent.contains("Safari/") && !user_agent.contains("Chrome/") {
            "Safari"
        } else {
            "Unknown"
        };

        let version = user_agent
            .split_whitespace()
            .find(|part| part.starts_with(ua))
            .unwrap_or("Unknown/12")
            .split('/')
            .nth(1)
            .unwrap_or("12")
            .split('.')
            .next()
            .unwrap_or("12");

        let ua = format!("\"Chromium\";v=\"{version}\",")
            + &match ua {
                "Edg" => format!("\"Microsoft Edge\";v=\"{version}\""),
                "Chromium" => format!("\"Google Chrome\";v=\"{version}\""),
                "Firefox" => format!("\"Firefox\";v=\"{version}\""),
                "Safari" => format!("\"Safari\";v=\"{version}\""),
                _ => format!("\"{ua}\";v=\"{version}\""),
            }
            + ", \"Not_A Brand\";v=\"99\"";

        HeaderMap::from_iter([
            (header::ORIGIN, "https://www.pixiv.net/".parse().unwrap()),
            (header::REFERER, "https://www.pixiv.net/".parse().unwrap()),
            (header::USER_AGENT, user_agent.parse().unwrap()),
            (header::DNT, "1".parse().unwrap()),
            (
                header::ACCEPT_LANGUAGE,
                "ja-JP,ja;q=0.9,en-US;q=0.8,en;q=0.7".parse().unwrap(),
            ),
            (
                header::ACCEPT,
                "application/json, text/plain, */*".parse().unwrap(),
            ),
            (
                header::HeaderName::from_static("sec-ch-ua"),
                ua.parse().unwrap(),
            ),
            (
                header::HeaderName::from_static("sec-ch-ua-platform"),
                platform.parse().unwrap(),
            ),
            (
                header::HeaderName::from_static("sec-ch-ua-mobile"),
                mobile.parse().unwrap(),
            ),
            (
                header::HeaderName::from_static("sec-fetch-dest"),
                "empty".parse().unwrap(),
            ),
            (
                header::HeaderName::from_static("sec-fetch-mode"),
                "cors".parse().unwrap(),
            ),
            (
                header::HeaderName::from_static("sec-fetch-site"),
                "same-site".parse().unwrap(),
            ),
        ])
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
