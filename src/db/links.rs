/// Article cross-reference link operations.
use std::collections::{HashSet, VecDeque};

use sqlx::SqlitePool;

use super::DbError;
use super::models::{ArticleLink, LinkGraphNode};

/// Add a link between two articles.
pub async fn add_link(
    pool: &SqlitePool,
    source_article_id: i64,
    target_article_id: i64,
    relationship: &str,
    strength: f64,
) -> Result<ArticleLink, DbError> {
    let result = sqlx::query_as::<_, ArticleLink>(
        "INSERT INTO article_links (source_article_id, target_article_id, relationship, strength)
         VALUES (?, ?, ?, ?)
         RETURNING *",
    )
    .bind(source_article_id)
    .bind(target_article_id)
    .bind(relationship)
    .bind(strength)
    .fetch_one(pool)
    .await;

    match result {
        Ok(link) => Ok(link),
        Err(sqlx::Error::Database(ref e)) if e.message().contains("UNIQUE") => {
            Err(DbError::DuplicateEntry(format!(
                "link already exists: {source_article_id} → {target_article_id}"
            )))
        }
        Err(e) => Err(DbError::Sqlx(e)),
    }
}

/// Get all links originating from an article.
pub async fn get_links_for_article(
    pool: &SqlitePool,
    article_id: i64,
) -> Result<Vec<ArticleLink>, DbError> {
    let links =
        sqlx::query_as::<_, ArticleLink>("SELECT * FROM article_links WHERE source_article_id = ?")
            .bind(article_id)
            .fetch_all(pool)
            .await?;
    Ok(links)
}

/// Get all articles linked to a given article (both directions).
pub async fn get_linked_articles(
    pool: &SqlitePool,
    article_id: i64,
) -> Result<Vec<ArticleLink>, DbError> {
    let links = sqlx::query_as::<_, ArticleLink>(
        "SELECT * FROM article_links WHERE source_article_id = ? OR target_article_id = ?",
    )
    .bind(article_id)
    .bind(article_id)
    .fetch_all(pool)
    .await?;
    Ok(links)
}

/// Maximum number of nodes the BFS will visit, preventing resource exhaustion
/// on pathologically connected graphs.
const MAX_GRAPH_NODES: usize = 10_000;

/// Build a link graph via BFS from a starting article, up to `depth` hops.
/// Visits at most [`MAX_GRAPH_NODES`] nodes to prevent unbounded memory usage.
pub async fn get_link_graph(
    pool: &SqlitePool,
    start_article_id: i64,
    depth: u32,
) -> Result<Vec<LinkGraphNode>, DbError> {
    let mut visited: HashSet<i64> = HashSet::new();
    let mut queue: VecDeque<(i64, u32)> = VecDeque::new();
    let mut graph: Vec<LinkGraphNode> = Vec::new();

    visited.insert(start_article_id);
    queue.push_back((start_article_id, 0));

    while let Some((current_id, current_depth)) = queue.pop_front() {
        if graph.len() >= MAX_GRAPH_NODES {
            break;
        }

        let links = get_linked_articles(pool, current_id).await?;

        let mut linked_ids: Vec<i64> = Vec::new();
        for link in &links {
            let neighbor = if link.source_article_id == current_id {
                link.target_article_id
            } else {
                link.source_article_id
            };

            linked_ids.push(neighbor);

            if current_depth < depth && visited.len() < MAX_GRAPH_NODES && visited.insert(neighbor)
            {
                queue.push_back((neighbor, current_depth + 1));
            }
        }

        graph.push(LinkGraphNode {
            article_id: current_id,
            linked_article_ids: linked_ids,
        });
    }

    Ok(graph)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::models::NewArticle;
    use crate::db::{articles, feeds};
    use crate::test_utils::db::test_pool;

    async fn setup_articles(pool: &SqlitePool, count: usize) -> Vec<i64> {
        let feed = feeds::add_feed(pool, "https://example.com/rss", None)
            .await
            .unwrap();
        let mut ids = Vec::new();
        for i in 0..count {
            let a = articles::insert_article(
                pool,
                &NewArticle {
                    feed_id: feed.id,
                    guid: Some(format!("guid-{i}")),
                    url: format!("https://example.com/article-{i}"),
                    title: format!("Article {i}"),
                    author: None,
                    published_at: None,
                    summary: None,
                    content: None,
                    content_hash: None,
                },
            )
            .await
            .unwrap();
            ids.push(a.id);
        }
        ids
    }

    #[tokio::test]
    async fn add_and_get_link() {
        let pool = test_pool().await;
        let ids = setup_articles(&pool, 2).await;

        let link = add_link(&pool, ids[0], ids[1], "related", 0.9)
            .await
            .unwrap();
        assert_eq!(link.source_article_id, ids[0]);
        assert_eq!(link.target_article_id, ids[1]);

        let links = get_links_for_article(&pool, ids[0]).await.unwrap();
        assert_eq!(links.len(), 1);
    }

    #[tokio::test]
    async fn duplicate_link() {
        let pool = test_pool().await;
        let ids = setup_articles(&pool, 2).await;

        add_link(&pool, ids[0], ids[1], "related", 0.9)
            .await
            .unwrap();
        let err = add_link(&pool, ids[0], ids[1], "similar", 0.5)
            .await
            .unwrap_err();
        assert!(matches!(err, DbError::DuplicateEntry(_)));
    }

    #[tokio::test]
    async fn bidirectional_retrieval() {
        let pool = test_pool().await;
        let ids = setup_articles(&pool, 2).await;

        add_link(&pool, ids[0], ids[1], "related", 0.9)
            .await
            .unwrap();

        // Both directions should see the link
        let from_source = get_linked_articles(&pool, ids[0]).await.unwrap();
        let from_target = get_linked_articles(&pool, ids[1]).await.unwrap();
        assert_eq!(from_source.len(), 1);
        assert_eq!(from_target.len(), 1);
    }

    #[tokio::test]
    async fn link_graph_bfs() {
        let pool = test_pool().await;
        let ids = setup_articles(&pool, 4).await;

        // Chain: 0 → 1 → 2 → 3
        add_link(&pool, ids[0], ids[1], "related", 1.0)
            .await
            .unwrap();
        add_link(&pool, ids[1], ids[2], "related", 1.0)
            .await
            .unwrap();
        add_link(&pool, ids[2], ids[3], "related", 1.0)
            .await
            .unwrap();

        // Depth 1: should reach 0 and 1
        let graph = get_link_graph(&pool, ids[0], 1).await.unwrap();
        let visited: HashSet<i64> = graph.iter().map(|n| n.article_id).collect();
        assert!(visited.contains(&ids[0]));
        assert!(visited.contains(&ids[1]));
        assert!(!visited.contains(&ids[2]));

        // Depth 3: should reach all
        let graph = get_link_graph(&pool, ids[0], 3).await.unwrap();
        let visited: HashSet<i64> = graph.iter().map(|n| n.article_id).collect();
        assert_eq!(visited.len(), 4);
    }

    #[tokio::test]
    async fn cascade_delete_feed_removes_links() {
        let pool = test_pool().await;
        let ids = setup_articles(&pool, 2).await;
        add_link(&pool, ids[0], ids[1], "related", 1.0)
            .await
            .unwrap();

        feeds::remove_feed(&pool, 1).await.unwrap();

        // Links should be gone (articles cascaded)
        let links = get_linked_articles(&pool, ids[0]).await.unwrap();
        assert!(links.is_empty());
    }

    #[tokio::test]
    async fn add_link_nonexistent_articles() {
        let pool = test_pool().await;
        let err = add_link(&pool, 999, 1000, "related", 1.0)
            .await
            .unwrap_err();
        assert!(matches!(err, DbError::Sqlx(_)));
    }

    #[tokio::test]
    async fn self_link() {
        let pool = test_pool().await;
        let ids = setup_articles(&pool, 1).await;
        let link = add_link(&pool, ids[0], ids[0], "self-reference", 1.0)
            .await
            .unwrap();
        assert_eq!(link.source_article_id, ids[0]);
        assert_eq!(link.target_article_id, ids[0]);
    }

    #[tokio::test]
    async fn get_links_for_article_empty() {
        let pool = test_pool().await;
        let ids = setup_articles(&pool, 1).await;
        let links = get_links_for_article(&pool, ids[0]).await.unwrap();
        assert!(links.is_empty());
    }

    #[tokio::test]
    async fn get_linked_articles_empty() {
        let pool = test_pool().await;
        let ids = setup_articles(&pool, 1).await;
        let links = get_linked_articles(&pool, ids[0]).await.unwrap();
        assert!(links.is_empty());
    }

    #[tokio::test]
    async fn get_linked_articles_nonexistent() {
        let pool = test_pool().await;
        let links = get_linked_articles(&pool, 999).await.unwrap();
        assert!(links.is_empty());
    }

    #[tokio::test]
    async fn multiple_links_from_same_source() {
        let pool = test_pool().await;
        let ids = setup_articles(&pool, 3).await;

        add_link(&pool, ids[0], ids[1], "related", 0.8)
            .await
            .unwrap();
        add_link(&pool, ids[0], ids[2], "similar", 0.6)
            .await
            .unwrap();

        let outgoing = get_links_for_article(&pool, ids[0]).await.unwrap();
        assert_eq!(outgoing.len(), 2);

        let all = get_linked_articles(&pool, ids[0]).await.unwrap();
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn link_graph_depth_zero() {
        let pool = test_pool().await;
        let ids = setup_articles(&pool, 2).await;
        add_link(&pool, ids[0], ids[1], "related", 1.0)
            .await
            .unwrap();

        // Depth 0: should return start node only, with its neighbors listed but not traversed
        let graph = get_link_graph(&pool, ids[0], 0).await.unwrap();
        assert_eq!(graph.len(), 1);
        assert_eq!(graph[0].article_id, ids[0]);
        // The link is listed but neighbor is not expanded
        assert_eq!(graph[0].linked_article_ids, vec![ids[1]]);
    }

    #[tokio::test]
    async fn link_graph_cycle() {
        let pool = test_pool().await;
        let ids = setup_articles(&pool, 3).await;

        // Cycle: 0 → 1 → 2 → 0
        add_link(&pool, ids[0], ids[1], "related", 1.0)
            .await
            .unwrap();
        add_link(&pool, ids[1], ids[2], "related", 1.0)
            .await
            .unwrap();
        add_link(&pool, ids[2], ids[0], "related", 1.0)
            .await
            .unwrap();

        // BFS should handle cycle without infinite loop
        let graph = get_link_graph(&pool, ids[0], 10).await.unwrap();
        let visited: HashSet<i64> = graph.iter().map(|n| n.article_id).collect();
        assert_eq!(visited.len(), 3);
    }

    #[tokio::test]
    async fn link_graph_isolated_node() {
        let pool = test_pool().await;
        let ids = setup_articles(&pool, 1).await;

        // No links — should return just the start node with empty neighbors
        let graph = get_link_graph(&pool, ids[0], 5).await.unwrap();
        assert_eq!(graph.len(), 1);
        assert_eq!(graph[0].article_id, ids[0]);
        assert!(graph[0].linked_article_ids.is_empty());
    }

    #[tokio::test]
    async fn link_graph_depth_exceeds_actual() {
        let pool = test_pool().await;
        let ids = setup_articles(&pool, 2).await;
        add_link(&pool, ids[0], ids[1], "related", 1.0)
            .await
            .unwrap();

        // Depth 100 but only 2 nodes — should return both without error
        let graph = get_link_graph(&pool, ids[0], 100).await.unwrap();
        let visited: HashSet<i64> = graph.iter().map(|n| n.article_id).collect();
        assert_eq!(visited.len(), 2);
    }

    #[tokio::test]
    async fn link_relationship_and_strength_stored() {
        let pool = test_pool().await;
        let ids = setup_articles(&pool, 2).await;

        let link = add_link(&pool, ids[0], ids[1], "contradicts", 0.42)
            .await
            .unwrap();
        assert_eq!(link.relationship, "contradicts");
        assert!((link.strength - 0.42).abs() < f64::EPSILON);
    }

    #[tokio::test]
    async fn reverse_link_allowed() {
        let pool = test_pool().await;
        let ids = setup_articles(&pool, 2).await;

        // A→B and B→A should both be allowed (different direction)
        add_link(&pool, ids[0], ids[1], "related", 1.0)
            .await
            .unwrap();
        add_link(&pool, ids[1], ids[0], "related", 1.0)
            .await
            .unwrap();

        let links = get_linked_articles(&pool, ids[0]).await.unwrap();
        assert_eq!(links.len(), 2);
    }
}
