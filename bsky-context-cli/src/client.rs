//! Native adapters for the core's I/O traits: reqwest for HTTP, tokio for time.

use core::time::Duration;
use std::time::Instant;

use bsky_context_core::api::{Clock, Fetch, FetchError, QuotesResponse, ThreadResponse};
use reqwest::header::RETRY_AFTER;
use serde::de::DeserializeOwned;

/// [`Fetch`] over a reqwest client against an XRPC base URL.
pub struct HttpFetch {
    client: reqwest::Client,
    base_url: String,
}

impl HttpFetch {
    /// Creates a client for the given base URL (e.g. the public `AppView`).
    ///
    /// # Errors
    ///
    /// Returns the reqwest error if the TLS backend cannot be initialized.
    pub fn new(base_url: impl Into<String>, timeout: Duration) -> reqwest::Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .user_agent(concat!("bsky-context/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self {
            client,
            base_url: base_url.into(),
        })
    }

    async fn get<T: DeserializeOwned>(
        &self,
        method: &str,
        query: &[(&str, &str)],
    ) -> Result<T, FetchError> {
        let url = format!("{}/xrpc/{method}", self.base_url);
        let response = self
            .client
            .get(url)
            .query(query)
            .send()
            .await
            .map_err(|e| transport_error(&e))?;
        let status = response.status();
        if !status.is_success() {
            let retry_after = response
                .headers()
                .get(RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.trim().parse::<u64>().ok())
                .map(Duration::from_secs);
            let message = response.text().await.unwrap_or_default();
            return Err(FetchError::Status {
                status: status.as_u16(),
                message,
                retry_after,
            });
        }
        response
            .json()
            .await
            .map_err(|e| FetchError::Decode(e.to_string()))
    }
}

fn transport_error(err: &reqwest::Error) -> FetchError {
    if err.is_timeout() {
        FetchError::Timeout
    } else {
        FetchError::Network(err.to_string())
    }
}

impl Fetch for HttpFetch {
    async fn get_post_thread(
        &self,
        uri: &str,
        depth: u32,
        parent_height: u32,
    ) -> Result<ThreadResponse, FetchError> {
        let depth = depth.to_string();
        let parent_height = parent_height.to_string();
        self.get(
            "app.bsky.feed.getPostThread",
            &[
                ("uri", uri),
                ("depth", &depth),
                ("parentHeight", &parent_height),
            ],
        )
        .await
    }

    async fn get_quotes(
        &self,
        uri: &str,
        limit: u32,
        cursor: Option<&str>,
    ) -> Result<QuotesResponse, FetchError> {
        let limit = limit.to_string();
        let mut query = vec![("uri", uri), ("limit", limit.as_str())];
        if let Some(cursor) = cursor {
            query.push(("cursor", cursor));
        }
        self.get("app.bsky.feed.getQuotes", &query).await
    }
}

/// [`Clock`] over the system clock and tokio timers.
pub struct TokioClock {
    start: Instant,
}

impl TokioClock {
    /// Starts the monotonic clock now.
    #[must_use]
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
        }
    }
}

impl Default for TokioClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for TokioClock {
    fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }

    fn now_rfc3339(&self) -> String {
        jiff::Timestamp::now().strftime("%FT%TZ").to_string()
    }

    async fn sleep(&self, duration: Duration) {
        tokio::time::sleep(duration).await;
    }
}
