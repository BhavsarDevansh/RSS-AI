use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A mock HTTP server for serving RSS feeds and article HTML.
pub struct MockFeedServer {
    server: MockServer,
}

impl MockFeedServer {
    /// Start a new mock feed server.
    pub async fn start() -> Self {
        Self {
            server: MockServer::start().await,
        }
    }

    /// Base URL of the mock server.
    pub fn url(&self) -> String {
        self.server.uri()
    }

    /// Mount an RSS/Atom feed at the given path.
    pub async fn mount_feed(&self, feed_path: &str, xml_body: &str) {
        Mock::given(method("GET"))
            .and(path(feed_path))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(xml_body)
                    .insert_header("content-type", "application/xml"),
            )
            .mount(&self.server)
            .await;
    }

    /// Mount an HTML article at the given path.
    pub async fn mount_article(&self, article_path: &str, html_body: &str) {
        Mock::given(method("GET"))
            .and(path(article_path))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string(html_body)
                    .insert_header("content-type", "text/html"),
            )
            .mount(&self.server)
            .await;
    }

    /// Mount a 500 Internal Server Error at the given path.
    pub async fn mount_server_error(&self, error_path: &str) {
        Mock::given(method("GET"))
            .and(path(error_path))
            .respond_with(ResponseTemplate::new(500))
            .mount(&self.server)
            .await;
    }

    /// Mount a 404 Not Found at the given path.
    pub async fn mount_not_found(&self, not_found_path: &str) {
        Mock::given(method("GET"))
            .and(path(not_found_path))
            .respond_with(ResponseTemplate::new(404))
            .mount(&self.server)
            .await;
    }

    /// Mount a 429 Too Many Requests at the given path.
    pub async fn mount_429(&self, rate_limit_path: &str) {
        Mock::given(method("GET"))
            .and(path(rate_limit_path))
            .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "60"))
            .mount(&self.server)
            .await;
    }

    /// Mount a 304 Not Modified at the given path.
    pub async fn mount_304(&self, not_modified_path: &str) {
        Mock::given(method("GET"))
            .and(path(not_modified_path))
            .respond_with(ResponseTemplate::new(304))
            .mount(&self.server)
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn serves_xml_feed() {
        let server = MockFeedServer::start().await;
        let xml = "<rss><channel><title>Test</title></channel></rss>";
        server.mount_feed("/feed.xml", xml).await;

        let resp = reqwest::get(format!("{}/feed.xml", server.url()))
            .await
            .unwrap();

        assert_eq!(resp.status(), 200);
        let body = resp.text().await.unwrap();
        assert!(body.contains("<rss>"));
    }

    #[tokio::test]
    async fn returns_error_status() {
        let server = MockFeedServer::start().await;
        server.mount_server_error("/broken").await;
        server.mount_not_found("/missing").await;
        server.mount_429("/limited").await;

        let r500 = reqwest::get(format!("{}/broken", server.url()))
            .await
            .unwrap();
        assert_eq!(r500.status(), 500);

        let r404 = reqwest::get(format!("{}/missing", server.url()))
            .await
            .unwrap();
        assert_eq!(r404.status(), 404);

        let r429 = reqwest::get(format!("{}/limited", server.url()))
            .await
            .unwrap();
        assert_eq!(r429.status(), 429);
    }

    #[tokio::test]
    async fn returns_304_not_modified() {
        let server = MockFeedServer::start().await;
        server.mount_304("/cached").await;

        let resp = reqwest::get(format!("{}/cached", server.url()))
            .await
            .unwrap();
        assert_eq!(resp.status(), 304);
    }
}
