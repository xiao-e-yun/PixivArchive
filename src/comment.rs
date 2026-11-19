use futures::future::join_all;
use log::error;
use post_archiver::Comment;
use serde::Deserialize;

use crate::api::PixivClient;

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
    client: &PixivClient,
    id: &str,
    is_novel: bool,
    is_root: bool,
) -> Vec<Comment> {
    // Pixiv rejects large page sizes with 400, so page through roots and replies.
    const ROOT_LIMIT: usize = 50;

    let ty = if is_novel { "novel" } else { "illust" };
    let mut comments = vec![];
    for page in 1.. {
        let url = match is_root {
            true => {
                let offset = (page - 1) * ROOT_LIMIT;
                format!(
                    "https://www.pixiv.net/ajax/{ty}s/comments/roots?{ty}_id={id}&offset={offset}&limit={ROOT_LIMIT}"
                )
            }
            false => {
                format!("https://www.pixiv.net/ajax/{ty}s/comments/replies?comment_id={id}&page={page}")
            }
        };

        let response: PixivComments = match client.fetch(&url).await {
            Ok(response) => response,
            Err(e) => {
                let cty = if is_root { "comments" } else { "replies" };
                error!("[artwork][comment] Failed fetch {ty} {id} {cty}: {e}");
                break;
            }
        };
        comments.extend(response.comments);
        if !response.has_next {
            break;
        }
    }

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
                comment
                    .stamp_id
                    .map(|id| format!("(Stamp {id})"))
                    .unwrap_or_default(),
            ]
            .join(" "),
            replies,
        }
    }))
    .await
}
