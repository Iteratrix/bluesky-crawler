//! Lenses: renderings of a [`ContextWeb`] tuned for different reasoning tasks.
//!
//! Each lens transforms a web into a string:
//!
//! - `tree`: indented threaded view (conversation flow)
//! - `linear`: chronological narrative (summarization)
//! - `by-author`: grouped by participant (argument analysis)
//! - `raw`: JSON graph (programmatic use)
//! - `stats`: summary statistics (quick overview)
//! - `threads`: thread listing sorted by size
//! - `highlights`: most notable posts and authors
//! - `neighborhood`: N-hop subgraph around a post
//! - `timeline`: time-windowed chronological view
//! - `search`: filter by text content or author

use core::fmt;
use core::str::FromStr;
use std::collections::{HashMap, HashSet, VecDeque};

use crate::model::{ContextWeb, Post, QuoteEdge};

mod by_author;
mod highlights;
mod linear;
mod neighborhood;
mod raw;
mod search;
mod stats;
mod threads;
mod timeline;
mod tree;

/// A lens together with its parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lens {
    /// Indented threaded view.
    Tree,
    /// Chronological narrative with cross-references.
    Linear,
    /// Posts grouped by participant.
    ByAuthor,
    /// The full JSON graph.
    Raw,
    /// Summary statistics.
    Stats,
    /// Threads sorted by size.
    Threads {
        /// How many threads to list.
        top: usize,
    },
    /// Most quoted, most replied, highest engagement.
    Highlights {
        /// How many entries per ranking.
        top: usize,
    },
    /// Posts within N quote hops of a target post.
    Neighborhood {
        /// The target post; the web's root when absent.
        uri: Option<String>,
        /// Maximum hop distance.
        hops: usize,
    },
    /// Posts inside a time window.
    Timeline {
        /// Inclusive lower bound, ISO 8601.
        after: Option<String>,
        /// Exclusive upper bound, ISO 8601.
        before: Option<String>,
    },
    /// Posts matching a text query and/or author handle substring.
    Search {
        /// Case-insensitive substring of the post text.
        query: Option<String>,
        /// Case-insensitive substring of the author handle.
        author: Option<String>,
    },
}

/// Default number of threads listed by [`Lens::Threads`].
pub const DEFAULT_THREADS_TOP: usize = 20;
/// Default number of entries per ranking in [`Lens::Highlights`].
pub const DEFAULT_HIGHLIGHTS_TOP: usize = 10;
/// Default hop distance for [`Lens::Neighborhood`].
pub const DEFAULT_HOPS: usize = 2;

/// The lens names accepted by [`LensKind::from_str`].
pub const NAMES: [&str; 10] = [
    "tree",
    "linear",
    "by-author",
    "raw",
    "stats",
    "threads",
    "highlights",
    "neighborhood",
    "timeline",
    "search",
];

/// A lens name without parameters, for parsing user input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LensKind {
    /// See [`Lens::Tree`].
    Tree,
    /// See [`Lens::Linear`].
    Linear,
    /// See [`Lens::ByAuthor`].
    ByAuthor,
    /// See [`Lens::Raw`].
    Raw,
    /// See [`Lens::Stats`].
    Stats,
    /// See [`Lens::Threads`].
    Threads,
    /// See [`Lens::Highlights`].
    Highlights,
    /// See [`Lens::Neighborhood`].
    Neighborhood,
    /// See [`Lens::Timeline`].
    Timeline,
    /// See [`Lens::Search`].
    Search,
}

/// Error for an unrecognized lens name.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown lens {0:?}; options: {options}", options = NAMES.join(", "))]
pub struct UnknownLens(pub String);

impl FromStr for LensKind {
    type Err = UnknownLens;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "tree" => Ok(Self::Tree),
            "linear" => Ok(Self::Linear),
            "by-author" => Ok(Self::ByAuthor),
            "raw" => Ok(Self::Raw),
            "stats" => Ok(Self::Stats),
            "threads" => Ok(Self::Threads),
            "highlights" => Ok(Self::Highlights),
            "neighborhood" => Ok(Self::Neighborhood),
            "timeline" => Ok(Self::Timeline),
            "search" => Ok(Self::Search),
            _ => Err(UnknownLens(s.to_owned())),
        }
    }
}

impl LensKind {
    /// Returns the lens name as accepted by [`LensKind::from_str`].
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Tree => "tree",
            Self::Linear => "linear",
            Self::ByAuthor => "by-author",
            Self::Raw => "raw",
            Self::Stats => "stats",
            Self::Threads => "threads",
            Self::Highlights => "highlights",
            Self::Neighborhood => "neighborhood",
            Self::Timeline => "timeline",
            Self::Search => "search",
        }
    }

    /// Returns the lens with default parameters.
    #[must_use]
    pub fn with_defaults(self) -> Lens {
        match self {
            Self::Tree => Lens::Tree,
            Self::Linear => Lens::Linear,
            Self::ByAuthor => Lens::ByAuthor,
            Self::Raw => Lens::Raw,
            Self::Stats => Lens::Stats,
            Self::Threads => Lens::Threads {
                top: DEFAULT_THREADS_TOP,
            },
            Self::Highlights => Lens::Highlights {
                top: DEFAULT_HIGHLIGHTS_TOP,
            },
            Self::Neighborhood => Lens::Neighborhood {
                uri: None,
                hops: DEFAULT_HOPS,
            },
            Self::Timeline => Lens::Timeline {
                after: None,
                before: None,
            },
            Self::Search => Lens::Search {
                query: None,
                author: None,
            },
        }
    }
}

impl fmt::Display for LensKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl Lens {
    /// Returns which kind of lens this is.
    #[must_use]
    pub fn kind(&self) -> LensKind {
        match self {
            Self::Tree => LensKind::Tree,
            Self::Linear => LensKind::Linear,
            Self::ByAuthor => LensKind::ByAuthor,
            Self::Raw => LensKind::Raw,
            Self::Stats => LensKind::Stats,
            Self::Threads { .. } => LensKind::Threads,
            Self::Highlights { .. } => LensKind::Highlights,
            Self::Neighborhood { .. } => LensKind::Neighborhood,
            Self::Timeline { .. } => LensKind::Timeline,
            Self::Search { .. } => LensKind::Search,
        }
    }
}

/// Renders a web through a lens.
///
/// # Examples
///
/// ```
/// use bsky_context_core::lens::{render, Lens};
/// use bsky_context_core::model::ContextWeb;
///
/// let web = ContextWeb::new("at://did:plc:a/app.bsky.feed.post/1", "2026-01-01T00:00:00Z");
/// let json = render(&web, &Lens::Raw);
/// assert!(json.contains("\"format_version\": 2"));
/// ```
#[must_use]
pub fn render(web: &ContextWeb, lens: &Lens) -> String {
    match lens {
        Lens::Tree => tree::render(web),
        Lens::Linear => linear::render(web),
        Lens::ByAuthor => by_author::render(web),
        Lens::Raw => raw::render(web),
        Lens::Stats => stats::render(web),
        Lens::Threads { top } => threads::render(web, *top),
        Lens::Highlights { top } => highlights::render(web, *top),
        Lens::Neighborhood { uri, hops } => neighborhood::render(web, uri.as_deref(), *hops),
        Lens::Timeline { after, before } => {
            timeline::render(web, after.as_deref(), before.as_deref())
        }
        Lens::Search { query, author } => search::render(web, query.as_deref(), author.as_deref()),
    }
}

/// Shortens an ISO timestamp to `YYYY-MM-DD HH:MM`; `?` when empty.
#[must_use]
pub fn short_time(iso: &str) -> String {
    if iso.is_empty() {
        return "?".to_owned();
    }
    iso.replace('T', " ").chars().take(16).collect()
}

/// Formats an author as `Display Name (@handle)` or `@handle`.
#[must_use]
pub fn author_name(post: &Post) -> String {
    let handle = &post.author.handle;
    let display_name = &post.author.display_name;
    if display_name.is_empty() {
        format!("@{handle}")
    } else {
        format!("{display_name} (@{handle})")
    }
}

/// Collapses newlines and truncates to `max_len` characters with an ellipsis.
#[must_use]
pub fn truncate(text: &str, max_len: usize) -> String {
    let text = text.replace('\n', " ");
    let text = text.trim();
    if text.chars().count() <= max_len {
        return text.to_owned();
    }
    let keep = max_len.saturating_sub(3);
    let mut out: String = text.chars().take(keep).collect();
    out.push_str("...");
    out
}

/// Maps parent URI to the URIs of its replies, in web order.
#[must_use]
pub fn build_children(web: &ContextWeb) -> HashMap<&str, Vec<&str>> {
    let mut children: HashMap<&str, Vec<&str>> = HashMap::new();
    for post in web.iter_posts() {
        let Some(parent) = &post.reply_parent else {
            continue;
        };
        children
            .entry(parent.as_str())
            .or_default()
            .push(post.uri.as_str());
    }
    children
}

/// Maps source URI to the number of posts quoting it.
#[must_use]
pub fn build_quotes_received(web: &ContextWeb) -> HashMap<&str, usize> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for QuoteEdge { source, .. } in &web.quote_edges {
        *counts.entry(source.as_str()).or_default() += 1;
    }
    counts
}

/// Breadth-first hop distance of every thread from `start_thread`, over
/// quote edges treated as undirected.
#[must_use]
pub fn thread_hop_distances<'a>(
    web: &'a ContextWeb,
    start_thread: &'a str,
) -> HashMap<&'a str, usize> {
    let mut adjacency: HashMap<&str, HashSet<&str>> = HashMap::new();
    for QuoteEdge {
        source_thread,
        target_thread,
        ..
    } in &web.quote_edges
    {
        adjacency
            .entry(source_thread.as_str())
            .or_default()
            .insert(target_thread.as_str());
        adjacency
            .entry(target_thread.as_str())
            .or_default()
            .insert(source_thread.as_str());
    }

    let mut distances = HashMap::from([(start_thread, 0)]);
    let mut queue = VecDeque::from([start_thread]);
    while let Some(thread) = queue.pop_front() {
        let distance = distances[thread];
        let Some(neighbors) = adjacency.get(thread) else {
            continue;
        };
        let mut neighbors: Vec<&str> = neighbors.iter().copied().collect();
        neighbors.sort_unstable();
        for neighbor in neighbors {
            if distances.contains_key(neighbor) {
                continue;
            }
            distances.insert(neighbor, distance + 1);
            queue.push_back(neighbor);
        }
    }
    distances
}

/// Walks up reply parents from the web's root to the earliest ancestor
/// present in the web.
#[must_use]
pub fn find_tree_root(web: &ContextWeb) -> &str {
    let mut uri = web.root_uri.as_str();
    while let Some(post) = web.get_post(uri) {
        let Some(parent) = post.reply_parent.as_deref() else {
            break;
        };
        if !web.has_post(parent) {
            break;
        }
        uri = parent;
    }
    uri
}

#[cfg(test)]
pub(crate) mod fixtures {
    use crate::model::{Author, ContextWeb, Post, QuoteEdge, Thread};

    pub(crate) const ROOT: &str = "at://did:plc:a/app.bsky.feed.post/1";
    pub(crate) const REPLY: &str = "at://did:plc:b/app.bsky.feed.post/2";
    pub(crate) const QUOTE: &str = "at://did:plc:c/app.bsky.feed.post/3";
    pub(crate) const REPLY_TO_QUOTE: &str = "at://did:plc:b/app.bsky.feed.post/4";

    pub(crate) fn author(did: &str, handle: &str, display_name: &str) -> Author {
        Author {
            did: did.into(),
            handle: handle.into(),
            display_name: display_name.into(),
        }
    }

    /// Builds a small web: root -> reply, root -> quote -> reply-to-quote.
    ///
    /// Thread 1: Alice's root post + Bob's direct reply.
    /// Thread 2: Carol's quote post + Bob's reply to the quote.
    pub(crate) fn test_web() -> ContextWeb {
        let mut web = ContextWeb::new(ROOT, "2026-01-01T00:00:00Z");

        let mut thread1 = Thread::new(ROOT);
        thread1.posts.insert(
            ROOT.into(),
            Post {
                like_count: 10,
                repost_count: 3,
                quote_count: 1,
                ..Post::new(
                    ROOT,
                    "c1",
                    author("did:plc:a", "alice.bsky.social", "Alice"),
                    "Original post",
                    "2026-01-15T10:00:00Z",
                )
            },
        );
        thread1.posts.insert(
            REPLY.into(),
            Post {
                reply_parent: Some(ROOT.into()),
                reply_root: Some(ROOT.into()),
                like_count: 2,
                ..Post::new(
                    REPLY,
                    "c2",
                    author("did:plc:b", "bob.bsky.social", "Bob"),
                    "Direct reply",
                    "2026-01-15T10:05:00Z",
                )
            },
        );

        let mut thread2 = Thread::new(QUOTE);
        thread2.posts.insert(
            QUOTE.into(),
            Post {
                embed_uri: Some(ROOT.into()),
                embed_type: Some("app.bsky.embed.record".into()),
                like_count: 5,
                ..Post::new(
                    QUOTE,
                    "c3",
                    author("did:plc:c", "carol.bsky.social", ""),
                    "Quote post",
                    "2026-01-15T10:08:00Z",
                )
            },
        );
        thread2.posts.insert(
            REPLY_TO_QUOTE.into(),
            Post {
                reply_parent: Some(QUOTE.into()),
                reply_root: Some(QUOTE.into()),
                like_count: 1,
                ..Post::new(
                    REPLY_TO_QUOTE,
                    "c4",
                    author("did:plc:b", "bob.bsky.social", "Bob"),
                    "Reply to quote",
                    "2026-01-15T10:12:00Z",
                )
            },
        );

        web.add_thread(thread1);
        web.add_thread(thread2);
        web.quote_edges = vec![QuoteEdge {
            source: ROOT.into(),
            target: QUOTE.into(),
            source_thread: ROOT.into(),
            target_thread: QUOTE.into(),
        }];
        web
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::{QUOTE, REPLY, REPLY_TO_QUOTE, ROOT, test_web};
    use super::*;

    #[test]
    fn lens_kind_parses_every_name() {
        for name in NAMES {
            let kind: LensKind = name.parse().unwrap();
            assert_eq!(kind.name(), name);
            assert_eq!(kind.with_defaults().kind(), kind);
        }
        let err = "nope".parse::<LensKind>().unwrap_err();
        assert!(err.to_string().contains("by-author"));
    }

    #[test]
    fn short_time_formats() {
        assert_eq!(short_time("2026-01-15T10:05:30.123Z"), "2026-01-15 10:05");
        assert_eq!(short_time(""), "?");
    }

    #[test]
    fn author_name_formats() {
        let web = test_web();
        assert_eq!(
            author_name(web.get_post(ROOT).unwrap()),
            "Alice (@alice.bsky.social)"
        );
        assert_eq!(
            author_name(web.get_post(QUOTE).unwrap()),
            "@carol.bsky.social"
        );
    }

    #[test]
    fn truncate_collapses_and_cuts() {
        assert_eq!(truncate("a\nb  ", 80), "a b");
        assert_eq!(truncate("abcdefghij", 8), "abcde...");
        assert_eq!(truncate("héllo wörld", 8), "héllo...");
    }

    #[test]
    fn children_and_quote_counts() {
        let web = test_web();
        let children = build_children(&web);
        assert_eq!(children[ROOT], vec![REPLY]);
        assert_eq!(children[QUOTE], vec![REPLY_TO_QUOTE]);
        assert!(!children.contains_key(REPLY));
        let quotes = build_quotes_received(&web);
        assert_eq!(quotes[ROOT], 1);
        assert!(!quotes.contains_key(QUOTE));
    }

    #[test]
    fn hop_distances_follow_quote_edges_both_ways() {
        let web = test_web();
        let from_root = thread_hop_distances(&web, ROOT);
        assert_eq!(from_root[ROOT], 0);
        assert_eq!(from_root[QUOTE], 1);
        let from_quote = thread_hop_distances(&web, QUOTE);
        assert_eq!(from_quote[ROOT], 1);
        let isolated = thread_hop_distances(&web, "at://nowhere");
        assert_eq!(isolated.len(), 1);
    }

    #[test]
    fn tree_root_walks_up_known_parents() {
        let mut web = test_web();
        assert_eq!(find_tree_root(&web), ROOT);
        web.root_uri = REPLY.into();
        assert_eq!(find_tree_root(&web), ROOT);
        web.root_uri = "at://missing".into();
        assert_eq!(find_tree_root(&web), "at://missing");
    }
}
