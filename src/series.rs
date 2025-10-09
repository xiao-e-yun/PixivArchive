use log::{debug, error, info};
use plyne::{Input, Output};
use post_archiver_utils::ArchiveClient;
use serde::Deserialize;
use tokio::{sync::mpsc::UnboundedSender, task::JoinSet};

use crate::{
    artwork::PixivArtworkId,
    config::{Config, Progress},
    fetch,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
pub enum PixivSeriesId {
    Illust(u64),
    Novel(u64),
}

impl PixivSeriesId {
    pub fn id(&self) -> u64 {
        match self {
            PixivSeriesId::Illust(id) => *id,
            PixivSeriesId::Novel(id) => *id,
        }
    }

    pub fn url(&self) -> String {
        match self {
            PixivSeriesId::Illust(id) => format!("https://www.pixiv.net/series/{id}"),
            PixivSeriesId::Novel(id) => format!("https://www.pixiv.net/novel/series/{id}"),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct PixivSeries {
    pub page: PixivSeriesPage,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PixivSeriesPage {
    pub total: u64,
    #[serde(default)]
    pub series: Vec<PixivIllustSeriesPageWork>,
    #[serde(default)]
    pub series_contents: Vec<PixivNovelSeriesPageWork>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PixivIllustSeriesPageWork {
    work_id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PixivNovelSeriesPageWork {
    id: String,
}

pub async fn reslove_series(
    mut series_pipeline: Output<PixivSeriesId>,
    artworks_pipeline: Input<PixivArtworkId>,
    config: &Config,
    client: &ArchiveClient,
) {
    let mut join_set = JoinSet::new();
    let pb = Progress::new(config.multi.clone(), "series");

    debug!("[series] Waiting for series to resolve");
    while let Some(series) = series_pipeline.recv().await {
        let pb = pb.clone();
        pb.inc_length(1);

        let client = client.clone();
        let tx = artworks_pipeline.clone();
        join_set.spawn(async move {
            reslove_series_single(client, tx, series).await;
            info!("[series] Resolved {}", series.id());
            pb.inc(1);
        });
    }

    if join_set.is_empty() {
        return;
    }
    join_set.join_all().await;

    info!("[series] Resolve finished ");
}

async fn reslove_series_single(
    client: ArchiveClient,
    tx: UnboundedSender<PixivArtworkId>,
    series: PixivSeriesId,
) {
    let id = series.id();

    let limit = match series {
        PixivSeriesId::Illust(_) => 12,
        PixivSeriesId::Novel(_) => 30,
    };

    let mut page = 0;
    let mut total = 1;

    while page * limit < total {
        page += 1;
        let series_url = match series {
            PixivSeriesId::Illust(_) => {
                format!("https://www.pixiv.net/ajax/series/{id}?lang=ja&p={page}")
            }
            PixivSeriesId::Novel(_) => {
                let order = (page - 1) * limit;
                format!(
                    "https://www.pixiv.net/ajax/novel/series_content/{id}?lang=ja&last_order={order}&order_by=asc"
                )
            }
        };

        let series = match fetch::<PixivSeries>(&client, &series_url).await {
            Ok(response) => response,
            Err(e) => {
                let ty = match &series {
                    PixivSeriesId::Illust(_) => "illust series",
                    PixivSeriesId::Novel(_) => "novel series",
                };
                error!("[series] Failed to fetch {ty} {id}: {e:?}");
                return;
            }
        };

        total = series.page.total;
        for artwork in series.page.series {
            tx.send(PixivArtworkId::Illust(artwork.work_id.parse().unwrap()))
                .unwrap();
        }

        for artwork in series.page.series_contents {
            tx.send(PixivArtworkId::Novel(artwork.id.parse().unwrap()))
                .unwrap();
        }
    }
}
