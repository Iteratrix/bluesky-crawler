//! Thread-level breadth-first crawler over `getPostThread` and `getQuotes`.
//!
//! Each thread (reply tree) is one unit of work: a single `getPostThread`
//! call fetches it whole. Quote posts discovered in a thread, and posts
//! returned by `getQuotes` for any post in the web, seed further threads.
//! Threads are deduplicated by root URI so the crawl terminates even on
//! heavily cross-quoted conversations.

use core::time::Duration;

use crate::model::ContextWeb;

/// Limits on a crawl.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrawlOptions {
    /// Stop once the web holds this many posts.
    pub max_nodes: usize,
    /// Stop expanding beyond this many quote hops from the start post.
    pub max_depth: Option<usize>,
    /// Stop after this much wall-clock time.
    pub timeout: Duration,
    /// Maximum API requests in flight at once.
    pub concurrency: usize,
}

impl Default for CrawlOptions {
    fn default() -> Self {
        Self {
            max_nodes: 2000,
            max_depth: None,
            timeout: Duration::from_secs(300),
            concurrency: 2,
        }
    }
}

/// A snapshot of crawl progress, reported after each unit of work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    /// Posts collected so far.
    pub node_count: usize,
    /// Reply and quote edges collected so far.
    pub edge_count: usize,
    /// Threads collected so far.
    pub thread_count: usize,
}

/// Why a crawl ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// Every reachable thread was fetched.
    Complete,
    /// [`CrawlOptions::max_nodes`] was reached.
    MaxNodes,
    /// [`CrawlOptions::timeout`] elapsed.
    Timeout,
}

/// The result of a crawl: the web plus how the crawl ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrawlResult {
    /// The crawled web.
    pub web: ContextWeb,
    /// Why the crawl stopped.
    pub stop_reason: StopReason,
    /// Work items still queued when the crawl stopped; zero when complete.
    pub pending: usize,
}
