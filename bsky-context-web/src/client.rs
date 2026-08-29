//! Browser adapters for the core's I/O traits: `fetch()` and JS timers.

use core::time::Duration;

use bsky_context_core::api::{
    Clock, Fetch, FetchError, PUBLIC_APPVIEW, QuotesResponse, ThreadResponse,
};
use futures::FutureExt;
use serde::de::DeserializeOwned;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// [`Fetch`] over the browser's `fetch()` against an XRPC base URL.
pub struct BrowserFetch {
    base_url: String,
}

impl BrowserFetch {
    /// Targets the public `AppView`.
    #[must_use]
    pub fn public() -> Self {
        Self {
            base_url: PUBLIC_APPVIEW.to_owned(),
        }
    }

    async fn get<T: DeserializeOwned>(
        &self,
        method: &str,
        query: &[(&str, &str)],
    ) -> Result<T, FetchError> {
        let url = format!("{}/xrpc/{method}", self.base_url);
        let request = gloo_net::http::Request::get(&url)
            .query(query.iter().copied())
            .build()
            .map_err(|e| FetchError::Network(e.to_string()))?;
        let send = request.send().fuse();
        let timeout = gloo_timers::future::sleep(REQUEST_TIMEOUT).fuse();
        futures::pin_mut!(send, timeout);
        let response = futures::select! {
            response = send => response.map_err(|e| FetchError::Network(e.to_string()))?,
            () = timeout => return Err(FetchError::Timeout),
        };
        let status = response.status();
        if !response.ok() {
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|s| s.trim().parse::<u64>().ok())
                .map(Duration::from_secs);
            let message = response.text().await.unwrap_or_default();
            return Err(FetchError::Status {
                status,
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

impl Fetch for BrowserFetch {
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

/// [`Clock`] over `Date.now()` and `setTimeout`.
pub struct BrowserClock {
    start_ms: f64,
}

impl BrowserClock {
    /// Starts the clock now.
    #[must_use]
    pub fn new() -> Self {
        Self {
            start_ms: js_sys::Date::now(),
        }
    }
}

impl Default for BrowserClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for BrowserClock {
    fn elapsed(&self) -> Duration {
        let elapsed_ms = (js_sys::Date::now() - self.start_ms).max(0.0);
        Duration::from_secs_f64(elapsed_ms / 1000.0)
    }

    fn now_rfc3339(&self) -> String {
        js_sys::Date::new_0().to_iso_string().into()
    }

    async fn sleep(&self, duration: Duration) {
        gloo_timers::future::sleep(duration).await;
    }
}
