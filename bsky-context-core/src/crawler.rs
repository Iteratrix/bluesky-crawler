//! Thread-level breadth-first crawler over `getPostThread` and `getQuotes`.
//!
//! Each thread (reply tree) is one unit of work: a single `getPostThread`
//! call fetches it whole. Quote posts discovered in a thread, and posts
//! returned by `getQuotes` for any post in the web, seed further threads.
//! Threads are deduplicated by root URI so the crawl terminates even on
//! heavily cross-quoted conversations.

use core::future::Future;
use core::time::Duration;
use std::cell::Cell;
use std::collections::{HashMap, HashSet, VecDeque};

use futures::StreamExt;
use futures::stream::FuturesUnordered;
use indexmap::IndexMap;

use crate::api::{
    Clock, Fetch, FetchError, PostView, QUOTES_PAGE_SIZE, QuotesResponse, THREAD_DEPTH, ThreadNode,
    ThreadResponse, ThreadViewPost,
};
use crate::model::{ContextWeb, FacetFeature, Post, QuoteEdge, Thread};
use crate::uri::{AtUriParts, post_uri_from_link, split_at_uri};

#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

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

/// Maximum attempts for a single API call before giving up.
const MAX_RETRIES: u32 = 5;

/// Base delay for exponential backoff between retries.
const BASE_DELAY: Duration = Duration::from_secs(1);

/// Crawls the context web reachable from a post.
///
/// Runs a breadth-first search over threads: each `getPostThread` call
/// ingests a whole reply tree, and every post in the web that has quotes is
/// expanded with `getQuotes`. Quote embeds and post links found in
/// rich-text facets seed further threads.
///
/// The crawl is infallible. Individual fetch failures are logged and
/// skipped, so a partial web comes back rather than an error. Pass
/// `existing` to merge into a previous crawl: posts keep their text while
/// engagement counts refresh, and `getQuotes` is skipped for posts whose
/// quote count has not grown.
///
/// # Examples
///
/// ```
/// use core::future::Future;
/// use core::time::Duration;
///
/// use bsky_context_core::api::{Clock, Fetch, FetchError, QuotesResponse, ThreadResponse};
/// use bsky_context_core::crawler::{CrawlOptions, StopReason, crawl};
///
/// struct Offline;
/// impl Fetch for Offline {
///     fn get_post_thread(
///         &self,
///         uri: &str,
///         _depth: u32,
///         _parent_height: u32,
///     ) -> impl Future<Output = Result<ThreadResponse, FetchError>> {
///         let uri = uri.to_owned();
///         async move { Err(FetchError::Status { status: 404, message: uri, retry_after: None }) }
///     }
///
///     fn get_quotes(
///         &self,
///         _uri: &str,
///         _limit: u32,
///         _cursor: Option<&str>,
///     ) -> impl Future<Output = Result<QuotesResponse, FetchError>> {
///         async { Ok(QuotesResponse { posts: Vec::new(), cursor: None }) }
///     }
/// }
///
/// struct Frozen;
/// impl Clock for Frozen {
///     fn elapsed(&self) -> Duration {
///         Duration::ZERO
///     }
///     fn now_rfc3339(&self) -> String {
///         "2026-01-01T00:00:00Z".to_owned()
///     }
///     fn sleep(&self, _duration: Duration) -> impl Future<Output = ()> {
///         async {}
///     }
/// }
///
/// let result = futures::executor::block_on(crawl(
///     &Offline,
///     &Frozen,
///     "at://did:plc:alice/app.bsky.feed.post/1",
///     &CrawlOptions::default(),
///     None,
///     &mut |_progress| {},
/// ));
/// assert_eq!(result.stop_reason, StopReason::Complete);
/// assert_eq!(result.web.node_count(), 0);
/// ```
pub async fn crawl<F: Fetch, C: Clock>(
    fetch: &F,
    clock: &C,
    start_uri: &str,
    options: &CrawlOptions,
    existing: Option<ContextWeb>,
    on_progress: &mut dyn FnMut(Progress),
) -> CrawlResult {
    let gate = RateGate::default();
    let mut state = CrawlState::new(clock, start_uri, options, existing);
    state.enqueue(start_uri.to_owned(), 0);

    let mut in_flight = FuturesUnordered::new();
    loop {
        while in_flight.len() < options.concurrency {
            let Some(job) = state.next_job(clock) else {
                break;
            };
            in_flight.push(run_job(fetch, clock, &gate, job));
        }
        let Some(outcome) = in_flight.next().await else {
            break;
        };
        state.apply(clock, outcome);
        on_progress(Progress {
            node_count: state.web.node_count(),
            edge_count: state.web.edge_count(),
            thread_count: state.web.thread_count(),
        });
    }

    state.finish(clock, start_uri)
}

/// One unit of crawl work.
enum Job {
    /// Fetch a post's reply tree.
    Thread {
        /// The post whose thread to fetch.
        uri: String,
        /// Quote hops from the start post.
        depth: usize,
    },
    /// Fetch every post quoting a post.
    Quotes {
        /// The quoted post.
        uri: String,
        /// Quote hops from the start post.
        depth: usize,
    },
}

/// What one [`Job`] fetched, ready to apply to the web.
enum JobOutcome {
    /// A thread response, or the failure that replaced it.
    Thread {
        /// The post whose thread was fetched.
        uri: String,
        /// Quote hops from the start post.
        depth: usize,
        /// The response, or the error that ended the retries.
        response: Result<ThreadResponse, FetchError>,
    },
    /// Quoting posts gathered across pages.
    Quotes {
        /// The quoted post.
        uri: String,
        /// Quote hops from the start post.
        depth: usize,
        /// Posts from every page that arrived.
        posts: Vec<PostView>,
        /// The failure that ended pagination early.
        error: Option<FetchError>,
    },
}

/// How long to back off, and whether every other request waits too.
enum Backoff {
    /// The server rate-limited us; all requests pause.
    RateLimited(Duration),
    /// A transient failure; only this request waits.
    Transient(Duration),
}

/// A shared pause on every request, held while backing off from a 429.
#[derive(Debug, Default)]
struct RateGate {
    until: Cell<Option<Duration>>,
}

impl RateGate {
    /// Waits until the gate is open.
    async fn wait<C: Clock>(&self, clock: &C) {
        while let Some(until) = self.until.get() {
            let Some(remaining) = until.checked_sub(clock.elapsed()) else {
                self.until.set(None);
                break;
            };
            if remaining.is_zero() {
                self.until.set(None);
                break;
            }
            clock.sleep(remaining).await;
        }
    }

    /// Closes the gate until the given point on the monotonic clock.
    fn close(&self, until: Duration) {
        self.until.set(Some(until));
    }

    /// Reopens the gate.
    fn open(&self) {
        self.until.set(None);
    }
}

/// Calls a request, retrying rate limits and transient failures.
///
/// A 429 pauses every other request through `gate` for the length of the
/// backoff; other HTTP statuses and decode failures give up immediately.
async fn retry<C, T, F, Fut>(clock: &C, gate: &RateGate, mut request: F) -> Result<T, FetchError>
where
    C: Clock,
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, FetchError>>,
{
    gate.wait(clock).await;
    let mut last = FetchError::Timeout;
    for attempt in 0..MAX_RETRIES {
        let error = match request().await {
            Ok(value) => return Ok(value),
            Err(error) => error,
        };
        let backoff = match &error {
            FetchError::Status {
                status: 429,
                retry_after,
                ..
            } => Backoff::RateLimited((*retry_after).unwrap_or(BASE_DELAY * 2u32.pow(attempt))),
            FetchError::Status { .. } | FetchError::Decode(_) => return Err(error),
            FetchError::Timeout | FetchError::Network(_) => {
                if attempt + 1 == MAX_RETRIES {
                    return Err(error);
                }
                Backoff::Transient(BASE_DELAY)
            }
        };
        let attempt_number = attempt + 1;
        match backoff {
            Backoff::RateLimited(delay) => {
                log::info!(
                    "Rate limited (429), pausing all requests for {delay:?} (attempt {attempt_number}/{MAX_RETRIES})"
                );
                gate.close(clock.elapsed() + delay);
                clock.sleep(delay).await;
                gate.open();
            }
            Backoff::Transient(delay) => {
                log::info!("{error}, retrying (attempt {attempt_number}/{MAX_RETRIES})");
                clock.sleep(delay).await;
            }
        }
        last = error;
    }
    Err(last)
}

/// Performs the I/O for one job, touching no crawl state.
async fn run_job<F: Fetch, C: Clock>(
    fetch: &F,
    clock: &C,
    gate: &RateGate,
    job: Job,
) -> JobOutcome {
    match job {
        Job::Thread { uri, depth } => {
            let response = retry(clock, gate, || {
                fetch.get_post_thread(&uri, THREAD_DEPTH, THREAD_DEPTH)
            })
            .await;
            JobOutcome::Thread {
                uri,
                depth,
                response,
            }
        }
        Job::Quotes { uri, depth } => {
            let (posts, error) = fetch_all_quotes(fetch, clock, gate, &uri).await;
            JobOutcome::Quotes {
                uri,
                depth,
                posts,
                error,
            }
        }
    }
}

/// Walks every page of `getQuotes`, keeping whatever arrived before a failure.
async fn fetch_all_quotes<F: Fetch, C: Clock>(
    fetch: &F,
    clock: &C,
    gate: &RateGate,
    uri: &str,
) -> (Vec<PostView>, Option<FetchError>) {
    let mut collected = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let page = retry(clock, gate, || {
            fetch.get_quotes(uri, QUOTES_PAGE_SIZE, cursor.as_deref())
        })
        .await;
        match page {
            Ok(QuotesResponse {
                posts,
                cursor: next,
            }) => {
                collected.extend(posts);
                let Some(next) = next else { break };
                cursor = Some(next);
            }
            Err(error) => return (collected, Some(error)),
        }
    }
    (collected, None)
}

/// Everything the crawl mutates, owned by the single sequential loop.
struct CrawlState {
    web: ContextWeb,
    queue: VecDeque<Job>,
    enqueued: HashSet<String>,
    visited_threads: HashSet<String>,
    visited_quotes: HashSet<String>,
    old_edge_counts: HashMap<String, u32>,
    handle_to_did: HashMap<String, String>,
    max_nodes: usize,
    max_depth: Option<usize>,
    deadline: Duration,
}

impl CrawlState {
    /// Builds the starting state, adopting `existing` when re-crawling.
    fn new<C: Clock>(
        clock: &C,
        start_uri: &str,
        options: &CrawlOptions,
        existing: Option<ContextWeb>,
    ) -> Self {
        let CrawlOptions {
            max_nodes,
            max_depth,
            timeout,
            concurrency: _,
        } = options;
        let mut old_edge_counts: HashMap<String, u32> = HashMap::new();
        let web = match existing {
            Some(mut web) => {
                for QuoteEdge { source, .. } in &web.quote_edges {
                    *old_edge_counts.entry(source.clone()).or_default() += 1;
                }
                web.crawled_at = clock.now_rfc3339();
                web
            }
            None => ContextWeb::new(start_uri, clock.now_rfc3339()),
        };
        Self {
            web,
            queue: VecDeque::new(),
            enqueued: HashSet::new(),
            visited_threads: HashSet::new(),
            visited_quotes: HashSet::new(),
            old_edge_counts,
            handle_to_did: HashMap::new(),
            max_nodes: *max_nodes,
            max_depth: *max_depth,
            deadline: clock.elapsed() + *timeout,
        }
    }

    /// Queues a thread fetch unless that URI was queued before.
    fn enqueue(&mut self, uri: String, depth: usize) {
        if self.enqueued.insert(uri.clone()) {
            self.queue.push_back(Job::Thread { uri, depth });
        }
    }

    /// Returns whether the node limit or the deadline has been reached.
    fn should_stop<C: Clock>(&self, clock: &C) -> bool {
        self.web.node_count() >= self.max_nodes || clock.elapsed() > self.deadline
    }

    /// Returns whether a hop distance is beyond the depth limit.
    fn over_depth(&self, depth: usize) -> bool {
        self.max_depth.is_some_and(|max| depth > max)
    }

    /// Pops the next job worth running, skipping ones already covered.
    fn next_job<C: Clock>(&mut self, clock: &C) -> Option<Job> {
        while !self.should_stop(clock) {
            let job = self.queue.pop_front()?;
            let Job::Thread { uri, depth } = &job else {
                return Some(job);
            };
            if self.over_depth(*depth) {
                continue;
            }
            let visited = self
                .web
                .thread_root_for(uri)
                .is_some_and(|root| self.visited_threads.contains(root));
            if !visited {
                return Some(job);
            }
        }
        None
    }

    /// Folds one job's data into the web.
    fn apply<C: Clock>(&mut self, clock: &C, outcome: JobOutcome) {
        match outcome {
            JobOutcome::Thread {
                uri,
                depth,
                response,
            } => {
                match response {
                    Ok(response) => {
                        if let Some(root) = self.ingest_thread(&uri, depth, &response) {
                            self.visited_threads.insert(root);
                        }
                    }
                    Err(error) => log::warn!("Failed to fetch thread for {uri}: {error}"),
                }
                self.scan_pending_quotes(clock, depth);
            }
            JobOutcome::Quotes {
                uri,
                depth,
                posts,
                error,
            } => {
                if let Some(error) = error {
                    log::warn!("Failed to fetch quotes for {uri}: {error}");
                }
                self.ingest_quotes(&uri, depth, posts);
            }
        }
    }

    /// Records a handle to DID mapping from a post's author.
    fn register_post(&mut self, post: &Post) {
        self.handle_to_did
            .insert(post.author.handle.clone(), post.author.did.clone());
    }

    /// Normalizes a handle-based AT URI to its canonical DID-based form.
    ///
    /// Returns the URI unchanged when it already names a post in the web,
    /// is not a three-segment AT URI, already uses a DID, or names a handle
    /// this crawl has not seen.
    fn resolve_uri(&self, uri: &str) -> String {
        if self.web.has_post(uri) {
            return uri.to_owned();
        }
        let Some(AtUriParts {
            authority,
            collection,
            rkey,
        }) = split_at_uri(uri)
        else {
            return uri.to_owned();
        };
        if authority.starts_with("did:") {
            return uri.to_owned();
        }
        let Some(did) = self.handle_to_did.get(authority) else {
            return uri.to_owned();
        };
        let canonical = format!("at://{did}/{collection}/{rkey}");
        if self.web.has_post(&canonical) {
            canonical
        } else {
            uri.to_owned()
        }
    }

    /// Ingests a thread response, returning the thread's root URI.
    fn ingest_thread(
        &mut self,
        uri: &str,
        depth: usize,
        response: &ThreadResponse,
    ) -> Option<String> {
        let mut posts = IndexMap::new();
        walk_thread_node(&response.thread, &mut posts);
        if posts.is_empty() {
            return None;
        }
        for post in posts.values() {
            self.register_post(post);
        }

        let root_uri = thread_root_of(&response.thread, &posts, uri);
        self.place_thread(&root_uri, &posts);
        for (post_uri, post) in &posts {
            match self.web.get_post_mut(post_uri) {
                Some(existing) => {
                    existing.like_count = post.like_count;
                    existing.reply_count = post.reply_count;
                    existing.repost_count = post.repost_count;
                    existing.quote_count = post.quote_count;
                }
                None => self.web.add_post(&root_uri, post.clone()),
            }
        }
        for post in posts.values() {
            self.link_quotes(post, &root_uri, depth);
        }
        Some(root_uri)
    }

    /// Makes sure a thread exists at `root_uri`, merging a thread whose
    /// posts turned out to belong under a different root.
    fn place_thread(&mut self, root_uri: &str, posts: &IndexMap<String, Post>) {
        let existing_root = posts
            .keys()
            .find_map(|post_uri| self.web.thread_root_for(post_uri))
            .map(str::to_owned);
        match existing_root {
            Some(existing_root) if existing_root != root_uri => {
                let old = self.web.remove_thread(&existing_root);
                if self.web.thread(root_uri).is_none() {
                    self.web.add_thread(Thread::new(root_uri));
                }
                for (post_uri, post) in old.into_iter().flat_map(|thread| thread.posts) {
                    if !self.web.has_post(&post_uri) {
                        self.web.add_post(root_uri, post);
                    }
                }
                for edge in &mut self.web.quote_edges {
                    if edge.source_thread == existing_root {
                        root_uri.clone_into(&mut edge.source_thread);
                    }
                    if edge.target_thread == existing_root {
                        root_uri.clone_into(&mut edge.target_thread);
                    }
                }
            }
            Some(_) => {}
            None => {
                if self.web.thread(root_uri).is_none() {
                    self.web.add_thread(Thread::new(root_uri));
                }
            }
        }
    }

    /// Records the quote edges a post declares, through its embed and
    /// through any link facet that points at another post.
    fn link_quotes(&mut self, post: &Post, root_uri: &str, depth: usize) {
        let embed_target = match &post.embed_uri {
            Some(embed_uri) => {
                let resolved = self.resolve_uri(embed_uri);
                self.add_quote_edge(&resolved, post, root_uri);
                self.follow_quote(&resolved, embed_uri, depth);
                resolved
            }
            None => String::new(),
        };
        for facet in &post.facets {
            for feature in &facet.features {
                let uri = match feature {
                    FacetFeature::Link { uri } => uri,
                    FacetFeature::Mention { .. }
                    | FacetFeature::Tag { .. }
                    | FacetFeature::Other { .. } => continue,
                };
                let Some(facet_uri) = post_uri_from_link(uri) else {
                    continue;
                };
                let resolved = self.resolve_uri(&facet_uri);
                if resolved == embed_target || resolved == post.uri {
                    continue;
                }
                self.add_quote_edge(&resolved, post, root_uri);
                self.follow_quote(&resolved, &facet_uri, depth);
            }
        }
    }

    /// Adds the edge from a quoted post to the post quoting it.
    fn add_quote_edge(&mut self, source: &str, target: &Post, target_thread: &str) {
        let source_thread = self
            .web
            .thread_root_for(source)
            .map_or_else(|| source.to_owned(), str::to_owned);
        self.web.quote_edges.push(QuoteEdge {
            source: source.to_owned(),
            target: target.uri.clone(),
            source_thread,
            target_thread: target_thread.to_owned(),
        });
    }

    /// Queues a quoted post's thread when the web does not hold it yet.
    fn follow_quote(&mut self, resolved: &str, uri: &str, depth: usize) {
        if self.web.thread_root_for(resolved).is_none() && !self.over_depth(depth + 1) {
            self.enqueue(uri.to_owned(), depth + 1);
        }
    }

    /// Ingests the posts quoting `uri`, each into its own reply thread.
    fn ingest_quotes(&mut self, uri: &str, depth: usize, posts: Vec<PostView>) {
        let source_thread = self
            .web
            .thread_root_for(uri)
            .map_or_else(|| uri.to_owned(), str::to_owned);
        for view in posts {
            let post = Post::from(view);
            self.register_post(&post);
            if self.web.has_post(&post.uri) {
                continue;
            }
            let target_thread = post.reply_root.clone().unwrap_or_else(|| post.uri.clone());
            if self.web.thread(&target_thread).is_none() {
                self.web.add_thread(Thread::new(&target_thread));
            }
            let target = post.uri.clone();
            self.web.add_post(&target_thread, post);
            self.web.quote_edges.push(QuoteEdge {
                source: uri.to_owned(),
                target,
                source_thread: source_thread.clone(),
                target_thread: target_thread.clone(),
            });
            if !self.over_depth(depth + 1) {
                self.enqueue(target_thread, depth + 1);
            }
        }
    }

    /// Queues a `getQuotes` pass for every post not yet checked for quotes.
    fn scan_pending_quotes<C: Clock>(&mut self, clock: &C, depth: usize) {
        let pending: Vec<(String, u32)> = self
            .web
            .iter_posts()
            .filter(|post| !self.visited_quotes.contains(&post.uri))
            .map(|post| (post.uri.clone(), post.quote_count))
            .collect();
        for (uri, quote_count) in pending {
            if self.should_stop(clock) {
                break;
            }
            self.visited_quotes.insert(uri.clone());
            if quote_count == 0 {
                continue;
            }
            if self
                .old_edge_counts
                .get(&uri)
                .is_some_and(|known| quote_count <= *known)
            {
                continue;
            }
            self.queue.push_back(Job::Quotes { uri, depth });
        }
    }

    /// Canonicalizes URIs, tidies the edges, and reports how the crawl ended.
    fn finish<C: Clock>(mut self, clock: &C, start_uri: &str) -> CrawlResult {
        let stop_reason = if clock.elapsed() > self.deadline {
            log::info!("Crawl stopped: timeout ({:?} limit)", self.deadline);
            StopReason::Timeout
        } else if self.web.node_count() >= self.max_nodes {
            log::info!("Crawl stopped: reached max_nodes ({})", self.max_nodes);
            StopReason::MaxNodes
        } else {
            log::info!(
                "Crawl complete: graph fully explored ({} posts)",
                self.web.node_count()
            );
            StopReason::Complete
        };

        let edges = core::mem::take(&mut self.web.quote_edges);
        self.web.quote_edges = edges
            .into_iter()
            .map(|edge| {
                let QuoteEdge {
                    source,
                    target,
                    source_thread,
                    target_thread,
                } = edge;
                QuoteEdge {
                    source: self.resolve_uri(&source),
                    target: self.resolve_uri(&target),
                    source_thread,
                    target_thread,
                }
            })
            .collect();
        self.web.normalize_quote_edges();

        let resolved = self.resolve_uri(start_uri);
        if self.web.has_post(&resolved) {
            self.web.root_uri = resolved;
        } else {
            let canonical = self.web.get_post(start_uri).map(|post| post.uri.clone());
            if let Some(canonical) = canonical {
                self.web.root_uri = canonical;
            }
        }

        CrawlResult {
            pending: self.queue.len(),
            web: self.web,
            stop_reason,
        }
    }
}

/// Collects every visible post in a thread view, parents and replies alike.
///
/// Nodes for posts that were deleted or blocked carry no post and are
/// skipped along with their subtrees.
fn walk_thread_node(node: &ThreadNode, posts: &mut IndexMap<String, Post>) {
    let Some(ThreadViewPost {
        post,
        parent,
        replies,
    }) = node.as_post()
    else {
        return;
    };
    let post = Post::from(post.clone());
    if !posts.contains_key(&post.uri) {
        posts.insert(post.uri.clone(), post);
    }
    if let Some(parent) = parent {
        walk_thread_node(parent, posts);
    }
    for reply in replies.iter().flatten() {
        walk_thread_node(reply, posts);
    }
}

/// Picks the root URI for a fetched thread.
///
/// Prefers the topmost ancestor the response carries, falling back to the
/// first collected post with no parent in the response, then to the URI
/// that was requested.
fn thread_root_of(node: &ThreadNode, posts: &IndexMap<String, Post>, requested: &str) -> String {
    find_response_root(node)
        .map(str::to_owned)
        .or_else(|| {
            posts
                .values()
                .find(|post| match &post.reply_parent {
                    Some(parent) => !posts.contains_key(parent),
                    None => true,
                })
                .map(|post| post.uri.clone())
        })
        .unwrap_or_else(|| requested.to_owned())
}

/// Walks up the parent chain to the topmost post in a thread response.
fn find_response_root(node: &ThreadNode) -> Option<&str> {
    let mut current = node;
    while let Some(view) = current.as_post() {
        let Some(parent) = view.parent.as_deref() else {
            break;
        };
        if parent.as_post().is_none() {
            break;
        }
        current = parent;
    }
    current.as_post().map(|view| view.post.uri.as_str())
}
