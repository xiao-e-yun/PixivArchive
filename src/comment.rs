use futures::future::join_all;
use log::error;
use post_archiver::Comment;
use post_archiver_utils::ArchiveClient;
use serde::Deserialize;

use crate::fetch;

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PixivComments {
    pub has_next: bool,
    pub comments: Vec<PixivComment>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PixivComment {
    pub user_id: String,
    pub user_name: String,
    pub img: String,
    pub id: String,
    pub comment_date: String,
    pub comment_parent_id: Option<String>,
    pub editable: bool,
    #[serde(default)]
    pub has_replies: bool,
    #[serde(rename = "comment")]
    pub content: String,
    pub stamp_id: Option<String>,
}

pub async fn get_comments(
    client: &ArchiveClient,
    id: &str,
    is_novel: bool,
    is_root: bool,
) -> Vec<Comment> {
    let ty = if is_novel { "novel" } else { "illust" };
    let url = match is_root {
        true => {
            format!("https://www.pixiv.net/ajax/{ty}s/comments/roots?{ty}_id={id}&limit=4294967295")
        }
        false => {
            format!("https://www.pixiv.net/ajax/{ty}s/comments/replies?comment_id={id}&page=1")
        }
    };

    let PixivComments { comments, .. } = fetch(client, &url)
        .await
        .inspect_err(|e| {
            let cty = if is_root { "comments" } else { "replies" };
            error!("[artwork][comment] Failed fetch {ty} {id} {cty}: {e:?}");
        })
        .unwrap_or_default();

    join_all(comments.into_iter().map(async |comment| {
        let replies = if comment.has_replies {
            get_comments(client, &comment.id, is_novel, false).await
        } else {
            vec![]
        };

        Comment {
            user: comment.user_name,
            text: [
                comment.content,
                comment.stamp_id.map(|id| format!("(Stamp {id})")).unwrap_or_default()
            ].join(" "),
            replies,
        }
    }))
    .await
}
