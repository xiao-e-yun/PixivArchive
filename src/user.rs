use std::{
    collections::{HashMap, hash_map::Entry},
    fmt::Debug,
    sync::Arc,
};

use log::{debug, error, info};
use post_archiver::{
    AuthorId, PlatformId,
    importer::{UnsyncAlias, UnsyncAuthor},
    manager::PostArchiverManager,
};
use post_archiver_utils::ArchiveClient;
use serde::Deserialize;
use tokio::sync::{
    Mutex, MutexGuard,
    mpsc::{UnboundedReceiver, UnboundedSender},
};
use tokio::task::JoinSet;

use crate::{
    NullableBody,
    artwork::{PixivArtwork, PixivArtworkId},
    config::{Config, ProgressManager},
    fetch,
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
    config: &Config,
    client: ArchiveClient,
    mut rx: UnboundedReceiver<PixivUserId>,
    tx: UnboundedSender<PixivArtworkId>,
) {
    let mut join_set = JoinSet::new();
    let pb = ProgressManager::new(config.multi.clone(), "user");

    debug!("[user] Waiting for user to resolve");
    while let Some(user) = rx.recv().await {
        let pb = pb.clone();
        pb.inc_length(1);

        let client = client.clone();
        let tx = tx.clone();
        join_set.spawn(async move {
            reslove_user(client, tx, user).await;
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

async fn reslove_user(client: ArchiveClient, tx: UnboundedSender<PixivArtworkId>, id: PixivUserId) {
    let url = format!("https://www.pixiv.net/ajax/user/{id}/profile/all?lang=ja");
    let user_artworks = match fetch::<PixivUserArtworks>(&client, &url).await {
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
    inner: Arc<Mutex<HashMap<String, AuthorId>>>,
}

impl UserManager {
    pub fn new(platform: PlatformId) -> Self {
        Self {
            platform,
            inner: Default::default(),
        }
    }
    pub async fn get<'a>(
        &self,
        manager: &MutexGuard<'a, PostArchiverManager>,
        artwork: &PixivArtwork,
    ) -> Option<AuthorId> {
        let mut inner = self.inner.lock().await;
        match inner.entry(artwork.user_id.clone()) {
            Entry::Occupied(occupied_entry) => Some(*occupied_entry.get()),
            Entry::Vacant(vacant_entry) => {
                match UnsyncAuthor::new(artwork.user_name.clone())
                    .aliases(vec![
                        UnsyncAlias::new(self.platform, artwork.user_id.clone())
                            .link(format!("https://www.pixiv.net/users/{}", artwork.user_id)),
                    ])
                    .sync(manager)
                {
                    Ok(author) => {
                        vacant_entry.insert(author);
                        Some(author)
                    }
                    Err(e) => {
                        error!(
                            "[user] Failed to create author for {}: {:?}",
                            artwork.user_id, e
                        );
                        None
                    }
                }
            }
        }
    }
}
