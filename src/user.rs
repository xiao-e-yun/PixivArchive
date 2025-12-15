use std::{
    collections::{HashMap, hash_map::Entry},
    fmt::Debug,
};

use log::{debug, error, info};
use plyne::{Input, Output};
use post_archiver::{
    AuthorId, PlatformId,
    importer::{UnsyncAlias, UnsyncAuthor},
    manager::PostArchiverManager,
};
use post_archiver_utils::{Error, Result};
use serde::Deserialize;
use tokio::sync::MutexGuard;
use tokio::task::JoinSet;

use crate::{
    api::{NullableBody, PixivClient},
    artwork::{PixivArtwork, PixivArtworkId},
    config::{Config, Progress},
};

pub type PixivUserId = u64;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PixivUserArtworks {
    pub illusts: NullableBody<HashMap<String, ()>>,
    pub manga: NullableBody<HashMap<String, ()>>,
    pub novels: NullableBody<HashMap<String, ()>>,
    // series are novel and manga of set
    // pub manga_series: HashMap<String, ()>,
    // pub novel_series: HashMap<String, ()>,
}

impl PixivUserArtworks {
    pub fn list(self) -> Vec<PixivArtworkId> {
        let mut artworks = vec![];

        if let NullableBody::Some(illusts) = self.illusts {
            artworks.extend(
                illusts
                    .into_keys()
                    .map(|v| v.parse().unwrap())
                    .map(PixivArtworkId::Illust),
            );
        };

        if let NullableBody::Some(mangas) = self.manga {
            artworks.extend(
                mangas
                    .into_keys()
                    .map(|v| v.parse().unwrap())
                    .map(PixivArtworkId::Illust),
            );
        };

        if let NullableBody::Some(novels) = self.novels {
            artworks.extend(
                novels
                    .into_keys()
                    .map(|v| v.parse().unwrap())
                    .map(PixivArtworkId::Novel),
            );
        };

        artworks
    }
}

pub async fn reslove_users(
    mut users_pipeline: Output<PixivUserId>,
    artworks_pipeline: Input<PixivArtworkId>,
    config: &Config,
    client: &PixivClient,
) {
    let mut join_set = JoinSet::new();
    let pb = Progress::new(config.multi.clone(), "user");

    debug!("[user] Waiting for user to resolve");
    while let Some(user) = users_pipeline.recv().await {
        let pb = pb.clone();
        pb.inc_length(1);

        let client = client.clone();
        let tx = artworks_pipeline.clone();
        join_set.spawn(async move {
            reslove_user(tx, client, user).await;
            info!("[user] Resolved {user}");
            pb.inc(1);
        });
    }

    if join_set.is_empty() {
        return;
    }
    join_set.join_all().await;

    info!("[user] Resolve finished");
}

async fn reslove_user(tx: Input<PixivArtworkId>, client: PixivClient, id: PixivUserId) {
    let url = format!("https://www.pixiv.net/ajax/user/{id}/profile/all?lang=ja");
    let user_artworks = match client.fetch::<PixivUserArtworks>(&url).await {
        Ok(artworks) => artworks,
        Err(e) => {
            error!("[user] Failed to fetch {id}: {e:?}");
            return;
        }
    };

    info!("[user] Resloved user {id}");
    if let NullableBody::Some(illusts) = &user_artworks.illusts {
        info!("  + {} illusts", illusts.len());
    }
    if let NullableBody::Some(mangas) = &user_artworks.manga {
        info!("  + {} mangas", mangas.len());
    }
    if let NullableBody::Some(novels) = &user_artworks.novels {
        info!("  + {} novels", novels.len());
    }

    for artwork in user_artworks.list() {
        tx.send(artwork).ok();
    }
}

#[derive(Debug, Clone)]
pub struct UserManager {
    pub platform: PlatformId,
    inner: HashMap<String, AuthorId>,
}

impl UserManager {
    pub fn new(platform: PlatformId) -> Self {
        Self {
            platform,
            inner: Default::default(),
        }
    }
    pub fn import(
        &mut self,
        manager: &MutexGuard<'_, PostArchiverManager>,
        artwork: &PixivArtwork,
    ) -> Result<AuthorId> {
        match self.inner.entry(artwork.user_id.clone()) {
            Entry::Occupied(occupied_entry) => Ok(*occupied_entry.get()),
            Entry::Vacant(vacant_entry) => UnsyncAuthor::new(artwork.user_name.clone())
                .aliases(vec![
                    UnsyncAlias::new(self.platform, artwork.user_id.clone())
                        .link(format!("https://www.pixiv.net/users/{}", artwork.user_id)),
                ])
                .sync(manager)
                .inspect(|id| {
                    vacant_entry.insert(*id);
                })
                .map_err(Error::from),
        }
    }
}
