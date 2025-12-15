use crate::{Config, api::PixivClient, artwork::PixivArtworkId, user::PixivUserId};

use log::{debug, error, info, warn};
use plyne::Input;
use serde::Deserialize;
use tokio::task::JoinSet;

#[derive(Debug, Clone, Deserialize)]
pub struct PixivUserStatusOuter {
    pub user_status: PixivUserStatus,
}

#[serde_with::serde_as]
#[derive(Debug, Clone, Deserialize)]
pub struct PixivUserStatus {
    #[serde_as(as = "serde_with::DisplayFromStr")]
    pub user_id: u64,
}

pub async fn reslove_current_user(
    users_pipeline: Input<PixivUserId>,
    artworks_pipeline: Input<PixivArtworkId>,
    client: &PixivClient,
    config: &Config,
) {
    if !(config.favorite || config.followed_users) {
        debug!("[current_user] Skipping favorites and following users archiving");
        return;
    }

    let user = match client.fetch::<PixivUserStatusOuter>(
        "https://www.pixiv.net/ajax/settings/self?lang=zh_tw",
    )
    .await
    {
        Ok(response) => response.user_status.user_id,
        Err(e) => {
            error!("[current_user] Failed to fetch current user: {e:?}");
            return;
        }
    };

    info!("[current_user] Current user ID: {user}");

    let mut join_set = JoinSet::new();
    if config.followed_users {
        info!("[following] Archiving followed users");
        join_set.spawn(reslove_following(users_pipeline, client.clone(), user));
    }

    if config.favorite {
        for ty in ["illusts", "novels"] {
            info!("[favorite] Fetching favorites of {ty}");
            let tx_artwork = artworks_pipeline.clone();
            join_set.spawn(reslove_favorite(tx_artwork, client.clone(), ty, user));
        }
    }

    join_set.join_all().await;
}

#[derive(Debug, Clone, Deserialize)]
pub struct PixivFavorite {
    pub total: usize,
    pub works: Vec<PixivFavoriteWork>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PixivFavoriteWork {
    pub id: PixivFavoriteWorkId,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum PixivFavoriteWorkId {
    Common(String),
    Unreachable(u64),
}

pub async fn reslove_favorite(
    tx: Input<PixivArtworkId>,
    client: PixivClient,
    ty: &'static str,
    user: u64,
) {
    let mut page = 0;
    let mut total = 1;
    const LIMIT: usize = 100;

    let mut offset = 0;
    while offset <= total {
        offset = page * LIMIT;
        page += 1;

        let url = format!(
            "https://www.pixiv.net/ajax/user/{user}/{ty}/bookmarks?tag=&offset={offset}&limit={LIMIT}&rest=show"
        );

        let response = match client.fetch::<PixivFavorite>(&url).await {
            Ok(response) => response,
            Err(e) => {
                error!("[favorite] Failed to fetch {ty}: {e:?}");
                return;
            }
        };
        total = response.total;

        for artwork in response.works {
            let id = match artwork.id {
                PixivFavoriteWorkId::Common(id) => id.parse::<u64>().unwrap(),
                PixivFavoriteWorkId::Unreachable(id) => {
                    warn!("[favorite] Unreachable favorite artwork {id}, skipping");
                    continue;
                }
            };
            let id = match ty {
                "illusts" => PixivArtworkId::Illust(id),
                "novels" => PixivArtworkId::Novel(id),
                _ => unreachable!("Invalid type for favorite: {ty}"),
            };
            info!("[favorite] Archive favorite artwork: {id:?}");
            tx.send(id).unwrap();
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct PixivFollowing {
    pub total: usize,
    pub users: Vec<PixivFollowingUser>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PixivFollowingUser {
    pub user_id: u64,
}
pub async fn reslove_following(tx: Input<PixivUserId>, client: PixivClient, user: u64) {
    let mut page = 0;
    let mut total = 1;
    const LIMIT: usize = 100;

    info!("[following] Fetching following user");
    let mut offset = 0;
    while offset <= total {
        offset = page * LIMIT;
        page += 1;

        let url = format!(
            "https://www.pixiv.net/ajax/user/{user}/following?tag=&offset={offset}&limit={LIMIT}&rest=show"
        );

        let response = match client.fetch::<PixivFollowing>(&url).await {
            Ok(response) => response,
            Err(e) => {
                error!("[following] Failed to fetch following user: {e:?}");
                return;
            }
        };
        total = response.total;
        for PixivFollowingUser { user_id } in response.users.iter() {
            info!("[following] Found following user: {user_id}");
            tx.send(*user_id).unwrap();
        }
    }
}
