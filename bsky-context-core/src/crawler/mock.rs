//! Test doubles for the crawler: a scripted [`Fetch`] and a fake [`Clock`].

use core::future::Future;
use core::time::Duration;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use crate::api::{
    Clock, Fetch, FetchError, Label, PostRecord, PostView, ProfileViewBasic, QuotesResponse,
    RecordEmbed, ReplyRef, StrongRef, ThreadNode, ThreadResponse, ThreadViewPost, WireFacet,
    WireFeature,
};
use crate::model::ByteRange;

/// How far the fake clock advances on every reading, standing in for the
/// forward motion of a real monotonic clock.
const TICK: Duration = Duration::from_millis(1);

/// Builds an AT URI from short names, as `at_uri("alice", "1")`.
pub fn at_uri(author: &str, rkey: &str) -> String {
    format!("at://did:plc:{author}/app.bsky.feed.post/{rkey}")
}

/// Which embed shape carries a quoted post.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbedKind {
    /// A plain quote post.
    Record,
    /// A quote post with attached media.
    RecordWithMedia,
}

/// Builds a [`PostView`] the way the `AppView` would present it.
pub struct PostViewBuilder {
    author: String,
    rkey: String,
    text: Option<String>,
    reply: Option<(String, String)>,
    embed: Option<(EmbedKind, String)>,
    facets: Option<Vec<WireFacet>>,
    like_count: u32,
    reply_count: u32,
    repost_count: u32,
    quote_count: u32,
    created_at: String,
}

/// Starts a post view for `author` with record key `rkey`.
pub fn make_post_view(author: &str, rkey: &str) -> PostViewBuilder {
    PostViewBuilder {
        author: author.to_owned(),
        rkey: rkey.to_owned(),
        text: None,
        reply: None,
        embed: None,
        facets: None,
        like_count: 0,
        reply_count: 0,
        repost_count: 0,
        quote_count: 0,
        created_at: "2026-01-01T00:00:00Z".to_owned(),
    }
}

impl PostViewBuilder {
    /// Sets the post text.
    pub fn text(mut self, text: &str) -> Self {
        self.text = Some(text.to_owned());
        self
    }

    /// Makes the post a reply, rooted at `root`.
    pub fn reply(mut self, parent: &str, root: &str) -> Self {
        self.reply = Some((parent.to_owned(), root.to_owned()));
        self
    }

    /// Makes the post quote another post.
    pub fn embed(mut self, uri: &str) -> Self {
        self.embed = Some((EmbedKind::Record, uri.to_owned()));
        self
    }

    /// Makes the post quote another post alongside media.
    pub fn embed_with_media(mut self, uri: &str) -> Self {
        self.embed = Some((EmbedKind::RecordWithMedia, uri.to_owned()));
        self
    }

    /// Attaches rich-text facets.
    pub fn facets(mut self, facets: Vec<WireFacet>) -> Self {
        self.facets = Some(facets);
        self
    }

    /// Sets the like count.
    pub fn like_count(mut self, count: u32) -> Self {
        self.like_count = count;
        self
    }

    /// Sets the reply count.
    pub fn reply_count(mut self, count: u32) -> Self {
        self.reply_count = count;
        self
    }

    /// Sets the quote count, which decides whether `getQuotes` runs.
    pub fn quote_count(mut self, count: u32) -> Self {
        self.quote_count = count;
        self
    }

    /// Finishes the post view.
    pub fn build(self) -> PostView {
        let Self {
            author,
            rkey,
            text,
            reply,
            embed,
            facets,
            like_count,
            reply_count,
            repost_count,
            quote_count,
            created_at,
        } = self;
        let uri = at_uri(&author, &rkey);
        let record = PostRecord {
            text: text.unwrap_or_else(|| format!("Post {rkey} by {author}")),
            created_at,
            reply: reply.map(|(parent, root)| ReplyRef {
                root: StrongRef { uri: root },
                parent: StrongRef { uri: parent },
            }),
            embed: embed.map(|(kind, uri)| match kind {
                EmbedKind::Record => RecordEmbed::Record {
                    record: StrongRef { uri },
                },
                EmbedKind::RecordWithMedia => RecordEmbed::RecordWithMedia {
                    record: crate::api::NestedRecordEmbed {
                        record: StrongRef { uri },
                    },
                },
            }),
            facets,
            langs: Some(Vec::new()),
        };
        PostView {
            uri,
            cid: format!("cid-{author}-{rkey}"),
            author: ProfileViewBasic {
                did: format!("did:plc:{author}"),
                handle: format!("{author}.bsky.social"),
                display_name: Some(capitalize(&author)),
            },
            record,
            labels: Some(Vec::<Label>::new()),
            reply_count: Some(reply_count),
            repost_count: Some(repost_count),
            like_count: Some(like_count),
            quote_count: Some(quote_count),
        }
    }
}

fn capitalize(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Builds a facet holding a single link feature.
pub fn make_link_facet(uri: &str) -> WireFacet {
    WireFacet {
        index: ByteRange {
            byte_start: 0,
            byte_end: 10,
        },
        features: vec![WireFeature {
            kind: "app.bsky.richtext.facet#link".to_owned(),
            did: None,
            uri: Some(uri.to_owned()),
            tag: None,
        }],
    }
}

/// Builds a thread view node around a post.
pub struct ThreadViewBuilder {
    post: PostView,
    parent: Option<ThreadNode>,
    replies: Vec<ThreadNode>,
}

/// Starts a thread view node for a post.
pub fn make_thread_view(post: PostView) -> ThreadViewBuilder {
    ThreadViewBuilder {
        post,
        parent: None,
        replies: Vec::new(),
    }
}

impl ThreadViewBuilder {
    /// Attaches the post this one replies to.
    pub fn parent(mut self, parent: ThreadNode) -> Self {
        self.parent = Some(parent);
        self
    }

    /// Attaches direct replies.
    pub fn replies(mut self, replies: Vec<ThreadNode>) -> Self {
        self.replies = replies;
        self
    }

    /// Finishes the node.
    pub fn build(self) -> ThreadNode {
        let Self {
            post,
            parent,
            replies,
        } = self;
        ThreadNode::Post(Box::new(ThreadViewPost {
            post,
            parent: parent.map(Box::new),
            replies: Some(replies),
        }))
    }
}

/// Builds a node standing in for a deleted post.
pub fn make_not_found(uri: &str) -> ThreadNode {
    ThreadNode::NotFound {
        uri: uri.to_owned(),
    }
}

/// Builds a node standing in for a post hidden by a block.
pub fn make_blocked(uri: &str) -> ThreadNode {
    ThreadNode::Blocked {
        uri: uri.to_owned(),
    }
}

/// One API call the crawler made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    /// A `getPostThread` call.
    PostThread {
        /// The requested post.
        uri: String,
        /// The requested reply depth.
        depth: u32,
        /// The requested ancestor height.
        parent_height: u32,
    },
    /// A `getQuotes` call.
    Quotes {
        /// The quoted post.
        uri: String,
        /// The requested page size.
        limit: u32,
        /// The page cursor, absent on the first page.
        cursor: Option<String>,
    },
}

/// A [`Fetch`] that answers from pre-registered responses and logs its calls.
#[derive(Default)]
pub struct MockFetch {
    threads: RefCell<HashMap<String, ThreadNode>>,
    quote_pages: RefCell<HashMap<String, Vec<Vec<PostView>>>>,
    quote_errors: RefCell<HashMap<String, FetchError>>,
    calls: RefCell<Vec<Call>>,
}

impl MockFetch {
    /// Creates a client that knows about no posts at all.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers the `getPostThread` response for a URI.
    pub fn add_thread(&self, uri: &str, node: ThreadNode) {
        self.threads.borrow_mut().insert(uri.to_owned(), node);
    }

    /// Registers the posts `getQuotes` returns for a URI, as one page.
    pub fn add_quotes(&self, uri: &str, posts: &[PostView]) {
        self.add_quotes_paged(uri, posts, 100);
    }

    /// Registers the posts `getQuotes` returns, split into pages.
    pub fn add_quotes_paged(&self, uri: &str, posts: &[PostView], page_size: usize) {
        let pages: Vec<Vec<PostView>> = posts
            .chunks(page_size)
            .map(<[PostView]>::to_vec)
            .collect::<Vec<_>>();
        let pages = if pages.is_empty() {
            vec![Vec::new()]
        } else {
            pages
        };
        self.quote_pages.borrow_mut().insert(uri.to_owned(), pages);
    }

    /// Makes `getQuotes` fail for a URI.
    pub fn set_quote_error(&self, uri: &str, error: FetchError) {
        self.quote_errors.borrow_mut().insert(uri.to_owned(), error);
    }

    /// Returns every call made, in order.
    pub fn calls(&self) -> Vec<Call> {
        self.calls.borrow().clone()
    }

    /// Returns the URIs passed to `getPostThread`, in order.
    pub fn thread_uris(&self) -> Vec<String> {
        self.calls()
            .into_iter()
            .filter_map(|call| match call {
                Call::PostThread { uri, .. } => Some(uri),
                Call::Quotes { .. } => None,
            })
            .collect()
    }

    /// Returns the URIs passed to `getQuotes`, in order.
    pub fn quote_uris(&self) -> Vec<String> {
        self.calls()
            .into_iter()
            .filter_map(|call| match call {
                Call::Quotes { uri, .. } => Some(uri),
                Call::PostThread { .. } => None,
            })
            .collect()
    }
}

impl Fetch for MockFetch {
    fn get_post_thread(
        &self,
        uri: &str,
        depth: u32,
        parent_height: u32,
    ) -> impl Future<Output = Result<ThreadResponse, FetchError>> {
        let uri = uri.to_owned();
        async move {
            self.calls.borrow_mut().push(Call::PostThread {
                uri: uri.clone(),
                depth,
                parent_height,
            });
            let threads = self.threads.borrow();
            let Some(thread) = threads.get(&uri) else {
                return Err(FetchError::Status {
                    status: 404,
                    message: format!("Post not found: {uri}"),
                    retry_after: None,
                });
            };
            Ok(ThreadResponse {
                thread: thread.clone(),
            })
        }
    }

    fn get_quotes(
        &self,
        uri: &str,
        limit: u32,
        cursor: Option<&str>,
    ) -> impl Future<Output = Result<QuotesResponse, FetchError>> {
        let uri = uri.to_owned();
        let cursor = cursor.map(str::to_owned);
        async move {
            self.calls.borrow_mut().push(Call::Quotes {
                uri: uri.clone(),
                limit,
                cursor: cursor.clone(),
            });
            if let Some(error) = self.quote_errors.borrow().get(&uri) {
                return Err(error.clone());
            }
            let pages = self.quote_pages.borrow();
            let Some(pages) = pages.get(&uri) else {
                return Ok(QuotesResponse {
                    posts: Vec::new(),
                    cursor: None,
                });
            };
            let index: usize = cursor.as_deref().map_or(0, |c| c.parse().unwrap_or(0));
            let posts = pages.get(index).cloned().unwrap_or_default();
            let next = if index + 1 < pages.len() {
                Some((index + 1).to_string())
            } else {
                None
            };
            Ok(QuotesResponse {
                posts,
                cursor: next,
            })
        }
    }
}

/// A [`Clock`] whose time only moves when read or slept on.
///
/// Readings advance by [`TICK`] so a zero timeout expires the way a real
/// monotonic clock would, and sleeps land instantly so backoff tests run
/// at full speed.
#[derive(Debug, Default)]
pub struct FakeClock {
    elapsed: Cell<Duration>,
    slept: Cell<Duration>,
}

impl FakeClock {
    /// Creates a clock reading zero.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the total time passed to [`Clock::sleep`].
    pub fn slept(&self) -> Duration {
        self.slept.get()
    }
}

impl Clock for FakeClock {
    fn elapsed(&self) -> Duration {
        let now = self.elapsed.get();
        self.elapsed.set(now + TICK);
        now
    }

    fn now_rfc3339(&self) -> String {
        "2026-01-01T00:00:00Z".to_owned()
    }

    async fn sleep(&self, duration: Duration) {
        self.elapsed.set(self.elapsed.get() + duration);
        self.slept.set(self.slept.get() + duration);
    }
}
