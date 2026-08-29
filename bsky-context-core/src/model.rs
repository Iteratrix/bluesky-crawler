//! Data model for the Context Web graph.
//!
//! A [`ContextWeb`] is a collection of [`Thread`]s (reply trees, the atomic
//! crawl unit) linked by [`QuoteEdge`]s. The JSON form is the storage
//! format shared with the original Python tool (`format_version` 2).

use std::collections::HashMap;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::uri::rkey_of;

/// Storage format version written into the `meta` envelope.
pub const FORMAT_VERSION: u32 = 2;

/// A post's author.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Author {
    /// Decentralized identifier, e.g. `did:plc:...`.
    pub did: String,
    /// Handle, e.g. `alice.bsky.social`.
    pub handle: String,
    /// Display name; empty when the profile has none.
    #[serde(default)]
    pub display_name: String,
}

/// A byte span inside post text, as used by rich-text facets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ByteRange {
    /// Inclusive start offset in UTF-8 bytes.
    pub byte_start: usize,
    /// Exclusive end offset in UTF-8 bytes.
    pub byte_end: usize,
}

/// One rich-text annotation feature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "RawFeature", into = "RawFeature")]
pub enum FacetFeature {
    /// A mention of another account.
    Mention {
        /// The mentioned account's DID.
        did: String,
    },
    /// A hyperlink.
    Link {
        /// The link target.
        uri: String,
    },
    /// A hashtag.
    Tag {
        /// The tag text without the `#`.
        tag: String,
    },
    /// A feature type this crate does not know.
    Other {
        /// The feature's `$type` NSID.
        kind: String,
    },
}

#[derive(Serialize, Deserialize)]
struct RawFeature {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    did: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    uri: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tag: Option<String>,
}

impl From<RawFeature> for FacetFeature {
    fn from(raw: RawFeature) -> Self {
        let RawFeature {
            kind,
            did,
            uri,
            tag,
        } = raw;
        match (kind.as_str(), did, uri, tag) {
            ("mention", Some(did), _, _) => Self::Mention { did },
            ("link", _, Some(uri), _) => Self::Link { uri },
            ("tag", _, _, Some(tag)) => Self::Tag { tag },
            _ => Self::Other { kind },
        }
    }
}

impl From<FacetFeature> for RawFeature {
    fn from(feature: FacetFeature) -> Self {
        let (kind, did, uri, tag) = match feature {
            FacetFeature::Mention { did } => ("mention", Some(did), None, None),
            FacetFeature::Link { uri } => ("link", None, Some(uri), None),
            FacetFeature::Tag { tag } => ("tag", None, None, Some(tag)),
            FacetFeature::Other { kind } => {
                return Self {
                    kind,
                    did: None,
                    uri: None,
                    tag: None,
                };
            }
        };
        Self {
            kind: kind.to_owned(),
            did,
            uri,
            tag,
        }
    }
}

/// A rich-text annotation over a byte span of the post text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Facet {
    /// The annotated span.
    pub index: ByteRange,
    /// The features applied to the span.
    pub features: Vec<FacetFeature>,
}

/// A single post node in the context web.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Post {
    /// Canonical AT URI.
    pub uri: String,
    /// Content identifier.
    pub cid: String,
    /// The author.
    pub author: Author,
    /// Post text.
    pub text: String,
    /// Creation time as an ISO 8601 / RFC 3339 string.
    pub created_at: String,
    /// URI of the post this one replies to.
    #[serde(default)]
    pub reply_parent: Option<String>,
    /// URI of the root of the reply thread this post belongs to.
    #[serde(default)]
    pub reply_root: Option<String>,
    /// Embed type NSID when the post quotes another post.
    #[serde(default)]
    pub embed_type: Option<String>,
    /// URI of the quoted post.
    #[serde(default)]
    pub embed_uri: Option<String>,
    /// Rich-text annotations.
    #[serde(default)]
    pub facets: Vec<Facet>,
    /// Moderation label values applied to the post.
    #[serde(default)]
    pub labels: Vec<String>,
    /// Declared languages.
    #[serde(default)]
    pub langs: Vec<String>,
    /// Like count at crawl time.
    #[serde(default)]
    pub like_count: u32,
    /// Reply count at crawl time.
    #[serde(default)]
    pub reply_count: u32,
    /// Repost count at crawl time.
    #[serde(default)]
    pub repost_count: u32,
    /// Quote count at crawl time.
    #[serde(default)]
    pub quote_count: u32,
}

impl Post {
    /// Creates a post with the required fields and every optional field empty.
    ///
    /// Use struct update syntax to set the rest.
    ///
    /// # Examples
    ///
    /// ```
    /// use bsky_context_core::model::{Author, Post};
    ///
    /// let author = Author { did: "did:plc:a".into(), handle: "a.bsky.social".into(), display_name: String::new() };
    /// let post = Post {
    ///     like_count: 3,
    ///     ..Post::new("at://did:plc:a/app.bsky.feed.post/1", "cid1", author, "hi", "2026-01-01T00:00:00Z")
    /// };
    /// assert_eq!(post.like_count, 3);
    /// assert!(post.reply_parent.is_none());
    /// ```
    #[must_use]
    pub fn new(
        uri: impl Into<String>,
        cid: impl Into<String>,
        author: Author,
        text: impl Into<String>,
        created_at: impl Into<String>,
    ) -> Self {
        Self {
            uri: uri.into(),
            cid: cid.into(),
            author,
            text: text.into(),
            created_at: created_at.into(),
            reply_parent: None,
            reply_root: None,
            embed_type: None,
            embed_uri: None,
            facets: Vec::new(),
            labels: Vec::new(),
            langs: Vec::new(),
            like_count: 0,
            reply_count: 0,
            repost_count: 0,
            quote_count: 0,
        }
    }

    /// Returns likes + reposts + quotes.
    #[must_use]
    pub fn engagement(&self) -> u32 {
        self.like_count + self.repost_count + self.quote_count
    }
}

/// A reply tree rooted at one post: the atomic crawl unit.
///
/// Posts keep insertion order, which lenses rely on for stable output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Thread {
    /// URI of the thread's root post.
    pub root_uri: String,
    /// Posts in the thread, keyed by URI.
    #[serde(default)]
    pub posts: IndexMap<String, Post>,
}

impl Thread {
    /// Creates an empty thread.
    #[must_use]
    pub fn new(root_uri: impl Into<String>) -> Self {
        Self {
            root_uri: root_uri.into(),
            posts: IndexMap::new(),
        }
    }

    /// Returns the number of posts in the thread.
    #[must_use]
    pub fn post_count(&self) -> usize {
        self.posts.len()
    }

    /// Returns the root post, if it has been fetched.
    #[must_use]
    pub fn root_post(&self) -> Option<&Post> {
        self.posts.get(&self.root_uri)
    }
}

/// A quote relationship between posts, possibly across threads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuoteEdge {
    /// URI of the quoted post.
    pub source: String,
    /// URI of the quoting post.
    pub target: String,
    /// Root URI of the thread containing `source`.
    pub source_thread: String,
    /// Root URI of the thread containing `target`.
    pub target_thread: String,
}

/// The complete crawled context graph: threads linked by quotes.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(from = "WebFile")]
pub struct ContextWeb {
    /// URI of the post the crawl started from.
    pub root_uri: String,
    /// When the crawl finished, as an RFC 3339 string.
    pub crawled_at: String,
    /// Quote edges, in discovery order. May contain duplicates until
    /// [`ContextWeb::normalize_quote_edges`] runs.
    pub quote_edges: Vec<QuoteEdge>,
    threads: IndexMap<String, Thread>,
    post_index: HashMap<String, String>,
}

impl ContextWeb {
    /// Creates an empty web.
    #[must_use]
    pub fn new(root_uri: impl Into<String>, crawled_at: impl Into<String>) -> Self {
        Self {
            root_uri: root_uri.into(),
            crawled_at: crawled_at.into(),
            quote_edges: Vec::new(),
            threads: IndexMap::new(),
            post_index: HashMap::new(),
        }
    }

    /// Registers a thread and indexes all its posts.
    ///
    /// Replaces any thread already stored under the same root URI.
    pub fn add_thread(&mut self, thread: Thread) {
        for uri in thread.posts.keys() {
            self.post_index.insert(uri.clone(), thread.root_uri.clone());
        }
        self.threads.insert(thread.root_uri.clone(), thread);
    }

    /// Removes a thread and deindexes its posts.
    pub fn remove_thread(&mut self, root_uri: &str) -> Option<Thread> {
        let thread = self.threads.shift_remove(root_uri)?;
        for uri in thread.posts.keys() {
            self.post_index.remove(uri);
        }
        Some(thread)
    }

    /// Adds a post to a thread, creating the thread if needed.
    pub fn add_post(&mut self, thread_root: &str, post: Post) {
        self.post_index
            .insert(post.uri.clone(), thread_root.to_owned());
        self.threads
            .entry(thread_root.to_owned())
            .or_insert_with(|| Thread::new(thread_root))
            .posts
            .insert(post.uri.clone(), post);
    }

    /// All threads, keyed by root URI, in insertion order.
    #[must_use]
    pub fn threads(&self) -> &IndexMap<String, Thread> {
        &self.threads
    }

    /// Looks up a thread by root URI.
    #[must_use]
    pub fn thread(&self, root_uri: &str) -> Option<&Thread> {
        self.threads.get(root_uri)
    }

    /// Returns the total number of posts.
    #[must_use]
    pub fn node_count(&self) -> usize {
        self.post_index.len()
    }

    /// Returns the number of reply edges plus quote edges.
    #[must_use]
    pub fn edge_count(&self) -> usize {
        self.reply_edge_count() + self.quote_edges.len()
    }

    fn reply_edge_count(&self) -> usize {
        self.iter_posts()
            .filter(|p| p.reply_parent.is_some())
            .count()
    }

    /// Returns the number of threads.
    #[must_use]
    pub fn thread_count(&self) -> usize {
        self.threads.len()
    }

    /// Iterates every post across all threads.
    pub fn iter_posts(&self) -> impl Iterator<Item = &Post> {
        self.threads.values().flat_map(|t| t.posts.values())
    }

    /// Returns a flat view of all posts keyed by URI, in thread order.
    #[must_use]
    pub fn nodes(&self) -> IndexMap<&str, &Post> {
        self.iter_posts().map(|p| (p.uri.as_str(), p)).collect()
    }

    /// Returns whether a post URI exists in any thread.
    #[must_use]
    pub fn has_post(&self, uri: &str) -> bool {
        self.post_index.contains_key(uri)
    }

    /// Looks up a post by URI.
    #[must_use]
    pub fn get_post(&self, uri: &str) -> Option<&Post> {
        let root = self.post_index.get(uri)?;
        self.threads.get(root)?.posts.get(uri)
    }

    /// Looks up a post by URI for mutation.
    pub fn get_post_mut(&mut self, uri: &str) -> Option<&mut Post> {
        let root = self.post_index.get(uri)?;
        self.threads.get_mut(root)?.posts.get_mut(uri)
    }

    /// Returns the root URI of the thread containing a post.
    #[must_use]
    pub fn thread_root_for(&self, uri: &str) -> Option<&str> {
        self.post_index.get(uri).map(String::as_str)
    }

    /// Returns the thread containing a post.
    #[must_use]
    pub fn thread_for_post(&self, uri: &str) -> Option<&Thread> {
        let root = self.post_index.get(uri)?;
        self.threads.get(root)
    }

    /// Returns the quote edges with stale thread refs fixed, orphans
    /// dropped, and duplicates removed, without modifying the web.
    #[must_use]
    pub fn normalized_quote_edges(&self) -> Vec<QuoteEdge> {
        let mut seen = std::collections::HashSet::new();
        let mut unique = Vec::new();
        for edge in &self.quote_edges {
            let QuoteEdge { source, target, .. } = edge;
            let (Some(source_thread), Some(target_thread)) =
                (self.post_index.get(source), self.post_index.get(target))
            else {
                continue;
            };
            if !seen.insert((source.clone(), target.clone())) {
                continue;
            }
            unique.push(QuoteEdge {
                source: source.clone(),
                target: target.clone(),
                source_thread: source_thread.clone(),
                target_thread: target_thread.clone(),
            });
        }
        unique
    }

    /// Fixes stale thread refs, drops orphan edges, and deduplicates in place.
    pub fn normalize_quote_edges(&mut self) {
        self.quote_edges = self.normalized_quote_edges();
    }

    /// Serializes the web as pretty-printed JSON in the storage format.
    ///
    /// # Panics
    ///
    /// Panics only if serialization fails, which cannot happen for this type.
    #[must_use]
    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).expect("ContextWeb serializes")
    }

    /// Parses a web from its JSON storage format.
    ///
    /// # Errors
    ///
    /// Returns the underlying [`serde_json::Error`] on malformed input.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }
}

#[derive(Serialize)]
struct MetaRef<'a> {
    format_version: u32,
    root_uri: &'a str,
    crawled_at: &'a str,
    node_count: usize,
    edge_count: usize,
    thread_count: usize,
}

#[derive(Serialize)]
struct WebFileRef<'a> {
    meta: MetaRef<'a>,
    threads: &'a IndexMap<String, Thread>,
    quote_edges: Vec<QuoteEdge>,
}

impl Serialize for ContextWeb {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let quote_edges = self.normalized_quote_edges();
        WebFileRef {
            meta: MetaRef {
                format_version: FORMAT_VERSION,
                root_uri: &self.root_uri,
                crawled_at: &self.crawled_at,
                node_count: self.node_count(),
                edge_count: self.reply_edge_count() + quote_edges.len(),
                thread_count: self.thread_count(),
            },
            threads: &self.threads,
            quote_edges,
        }
        .serialize(serializer)
    }
}

#[derive(Deserialize)]
struct Meta {
    root_uri: String,
    crawled_at: String,
}

#[derive(Deserialize)]
struct WebFile {
    meta: Meta,
    #[serde(default)]
    threads: IndexMap<String, Thread>,
    #[serde(default)]
    quote_edges: Vec<QuoteEdge>,
}

impl From<WebFile> for ContextWeb {
    fn from(file: WebFile) -> Self {
        let WebFile {
            meta: Meta {
                root_uri,
                crawled_at,
            },
            threads,
            quote_edges,
        } = file;
        let mut web = Self::new(root_uri, crawled_at);
        for (_, thread) in threads {
            web.add_thread(thread);
        }
        web.quote_edges = quote_edges;
        web
    }
}

/// Derives a short, deterministic identifier for a web from its root URI.
///
/// The form is `{rkey}-{sha256 prefix}`: readable, and collision resistant
/// across repositories that reuse record keys.
///
/// # Examples
///
/// ```
/// let id = bsky_context_core::model::web_id("at://did:plc:test/app.bsky.feed.post/abc123");
/// assert!(id.starts_with("abc123-"));
/// assert_eq!(id.len(), "abc123-".len() + 6);
/// ```
#[must_use]
pub fn web_id(root_uri: &str) -> String {
    let digest = Sha256::digest(root_uri.as_bytes());
    format!(
        "{}-{:02x}{:02x}{:02x}",
        rkey_of(root_uri),
        digest[0],
        digest[1],
        digest[2]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn author() -> Author {
        Author {
            did: "did:plc:test".into(),
            handle: "test.bsky.social".into(),
            display_name: String::new(),
        }
    }

    fn make_post(uri: &str) -> Post {
        let cid = format!("cid-{}", &uri[uri.len() - 3..]);
        Post::new(uri, cid, author(), "hello", "2026-01-01T00:00:00Z")
    }

    fn reply(uri: &str, parent: &str) -> Post {
        Post {
            reply_parent: Some(parent.into()),
            reply_root: Some(parent.into()),
            ..make_post(uri)
        }
    }

    fn single_thread(uri: &str) -> Thread {
        let mut t = Thread::new(uri);
        t.posts.insert(uri.into(), make_post(uri));
        t
    }

    fn empty_web() -> ContextWeb {
        ContextWeb::new("at://x/app.bsky.feed.post/1", "2026-01-01T00:00:00Z")
    }

    fn json_roundtrip<T: Serialize + for<'de> Deserialize<'de>>(value: &T) -> T {
        serde_json::from_str(&serde_json::to_string(value).unwrap()).unwrap()
    }

    #[test]
    fn author_roundtrip_and_default_display_name() {
        let a = Author {
            did: "did:plc:x".into(),
            handle: "x.bsky.social".into(),
            display_name: "X".into(),
        };
        assert_eq!(json_roundtrip(&a), a);
        let a: Author = serde_json::from_str(r#"{"did":"d","handle":"h"}"#).unwrap();
        assert_eq!(a.display_name, "");
    }

    #[test]
    fn post_roundtrip() {
        let p = Post {
            facets: vec![Facet {
                index: ByteRange {
                    byte_start: 0,
                    byte_end: 5,
                },
                features: vec![
                    FacetFeature::Link {
                        uri: "https://example.com".into(),
                    },
                    FacetFeature::Mention {
                        did: "did:plc:m".into(),
                    },
                    FacetFeature::Tag { tag: "rust".into() },
                    FacetFeature::Other {
                        kind: "app.bsky.richtext.facet#future".into(),
                    },
                ],
            }],
            reply_parent: Some("at://did:plc:a/app.bsky.feed.post/1".into()),
            ..make_post("at://did:plc:a/app.bsky.feed.post/2")
        };
        assert_eq!(json_roundtrip(&p), p);
        let json = serde_json::to_value(&p).unwrap();
        assert_eq!(json["facets"][0]["index"]["byteStart"], 0);
        assert_eq!(json["facets"][0]["features"][0]["type"], "link");
        assert_eq!(
            json["facets"][0]["features"][3]["type"],
            "app.bsky.richtext.facet#future"
        );
        assert!(json["facets"][0]["features"][0].get("did").is_none());
    }

    #[test]
    fn post_optional_fields_default() {
        let p: Post = serde_json::from_str(
            r#"{"uri":"u","cid":"c","author":{"did":"d","handle":"h"},"text":"t","created_at":"x"}"#,
        )
        .unwrap();
        assert!(p.reply_parent.is_none());
        assert!(p.embed_uri.is_none());
        assert!(p.facets.is_empty());
        assert_eq!(p.like_count, 0);
    }

    #[test]
    fn thread_counts_and_root() {
        let t = Thread::new("at://did:plc:a/app.bsky.feed.post/1");
        assert_eq!(t.post_count(), 0);
        assert!(t.root_post().is_none());

        let root = "at://did:plc:a/app.bsky.feed.post/1";
        let reply_uri = "at://did:plc:b/app.bsky.feed.post/2";
        let mut t = single_thread(root);
        t.posts.insert(reply_uri.into(), reply(reply_uri, root));
        assert_eq!(t.post_count(), 2);
        assert_eq!(t.root_post().unwrap().uri, root);
        let t2 = json_roundtrip(&t);
        assert_eq!(t2.posts[reply_uri].reply_parent.as_deref(), Some(root));
    }

    #[test]
    fn quote_edge_roundtrip() {
        let qe = QuoteEdge {
            source: "at://a/app.bsky.feed.post/1".into(),
            target: "at://b/app.bsky.feed.post/2".into(),
            source_thread: "at://a/app.bsky.feed.post/1".into(),
            target_thread: "at://b/app.bsky.feed.post/2".into(),
        };
        assert_eq!(json_roundtrip(&qe), qe);
    }

    #[test]
    fn empty_web_counts() {
        let web = empty_web();
        assert_eq!(web.node_count(), 0);
        assert_eq!(web.edge_count(), 0);
        assert_eq!(web.thread_count(), 0);
    }

    #[test]
    fn web_roundtrip_with_meta() {
        let mut web = empty_web();
        let root = "at://x/app.bsky.feed.post/1";
        let reply_uri = "at://x/app.bsky.feed.post/2";
        let mut t = single_thread(root);
        t.posts.insert(reply_uri.into(), reply(reply_uri, root));
        web.add_thread(t);

        let json = serde_json::to_value(&web).unwrap();
        assert_eq!(json["meta"]["format_version"], 2);
        assert_eq!(json["meta"]["node_count"], 2);
        assert_eq!(json["meta"]["edge_count"], 1);
        assert_eq!(json["meta"]["thread_count"], 1);
        assert_eq!(json["meta"]["root_uri"], root);

        let web2 = ContextWeb::from_json(&web.to_json_pretty()).unwrap();
        assert_eq!(web2, web);
        assert_eq!(web2.node_count(), 2);
        assert_eq!(web2.thread_root_for(reply_uri), Some(root));
    }

    #[test]
    fn reads_python_format() {
        let json = r#"{
          "meta": {"format_version": 2, "root_uri": "at://did:plc:a/app.bsky.feed.post/1",
                   "crawled_at": "2026-03-01T00:00:00+00:00", "node_count": 1, "edge_count": 0, "thread_count": 1},
          "threads": {
            "at://did:plc:a/app.bsky.feed.post/1": {
              "root_uri": "at://did:plc:a/app.bsky.feed.post/1",
              "posts": {
                "at://did:plc:a/app.bsky.feed.post/1": {
                  "uri": "at://did:plc:a/app.bsky.feed.post/1", "cid": "c", 
                  "author": {"did": "did:plc:a", "handle": "a.bsky.social", "display_name": "A"},
                  "text": "hi", "created_at": "2026-02-01T00:00:00.000Z",
                  "reply_parent": null, "reply_root": null, "embed_type": null, "embed_uri": null,
                  "facets": [{"index": {"byteStart": 0, "byteEnd": 2}, "features": [{"type": "tag", "tag": "x"}]}],
                  "labels": [], "langs": ["en"], "like_count": 4, "reply_count": 0, "repost_count": 1, "quote_count": 0
                }
              }
            }
          },
          "quote_edges": []
        }"#;
        let web = ContextWeb::from_json(json).unwrap();
        assert_eq!(web.node_count(), 1);
        let post = web.get_post("at://did:plc:a/app.bsky.feed.post/1").unwrap();
        assert_eq!(post.like_count, 4);
        assert_eq!(post.langs, vec!["en"]);
        assert_eq!(
            post.facets[0].features[0],
            FacetFeature::Tag { tag: "x".into() }
        );
    }

    #[test]
    fn counts_across_threads_and_nodes_view() {
        let mut web = empty_web();
        web.add_thread(single_thread("at://a/app.bsky.feed.post/1"));
        let mut t = single_thread("at://b/app.bsky.feed.post/2");
        t.posts.insert(
            "at://b/app.bsky.feed.post/3".into(),
            make_post("at://b/app.bsky.feed.post/3"),
        );
        web.add_thread(t);
        assert_eq!(web.node_count(), 3);
        assert_eq!(web.thread_count(), 2);
        let nodes = web.nodes();
        assert_eq!(nodes.len(), 3);
        assert!(nodes.contains_key("at://b/app.bsky.feed.post/3"));
        assert_eq!(nodes.keys().next(), Some(&"at://a/app.bsky.feed.post/1"));
    }

    #[test]
    fn lookups() {
        let mut web = empty_web();
        let root = "at://a/app.bsky.feed.post/1";
        web.add_thread(single_thread(root));
        assert!(web.has_post(root));
        assert_eq!(web.thread_for_post(root).unwrap().root_uri, root);
        assert!(web.thread_for_post("at://nonexistent").is_none());
        assert!(web.get_post("at://nonexistent").is_none());
        web.get_post_mut(root).unwrap().like_count = 7;
        assert_eq!(web.get_post(root).unwrap().like_count, 7);
    }

    #[test]
    fn add_post_creates_thread_and_remove_deindexes() {
        let mut web = empty_web();
        let root = "at://a/app.bsky.feed.post/1";
        web.add_post(root, make_post(root));
        assert_eq!(web.thread_count(), 1);
        assert_eq!(web.thread_root_for(root), Some(root));
        let removed = web.remove_thread(root).unwrap();
        assert_eq!(removed.post_count(), 1);
        assert!(!web.has_post(root));
        assert!(web.remove_thread(root).is_none());
    }

    #[test]
    fn normalize_deduplicates() {
        let mut web = empty_web();
        let a = "at://a/app.bsky.feed.post/1";
        let b = "at://b/app.bsky.feed.post/2";
        web.add_thread(single_thread(a));
        web.add_thread(single_thread(b));
        let qe = QuoteEdge {
            source: a.into(),
            target: b.into(),
            source_thread: a.into(),
            target_thread: b.into(),
        };
        web.quote_edges = vec![qe.clone(), qe.clone(), qe];
        web.normalize_quote_edges();
        assert_eq!(web.quote_edges.len(), 1);
    }

    #[test]
    fn normalize_preserves_distinct_edges() {
        let mut web = empty_web();
        let a = "at://a/app.bsky.feed.post/1";
        let b = "at://b/app.bsky.feed.post/2";
        let c = "at://c/app.bsky.feed.post/3";
        for uri in [a, b, c] {
            web.add_thread(single_thread(uri));
        }
        web.quote_edges = vec![
            QuoteEdge {
                source: a.into(),
                target: b.into(),
                source_thread: a.into(),
                target_thread: b.into(),
            },
            QuoteEdge {
                source: a.into(),
                target: c.into(),
                source_thread: a.into(),
                target_thread: c.into(),
            },
        ];
        web.normalize_quote_edges();
        assert_eq!(web.quote_edges.len(), 2);
    }

    #[test]
    fn normalize_fixes_stale_thread_refs() {
        let mut web = empty_web();
        let a = "at://a/app.bsky.feed.post/1";
        let b = "at://b/app.bsky.feed.post/2";
        let mut t = single_thread(a);
        t.posts.insert(b.into(), make_post(b));
        web.add_thread(t);
        web.quote_edges = vec![QuoteEdge {
            source: a.into(),
            target: b.into(),
            source_thread: "at://stale/placeholder".into(),
            target_thread: "at://stale/other".into(),
        }];
        web.normalize_quote_edges();
        assert_eq!(web.quote_edges.len(), 1);
        assert_eq!(web.quote_edges[0].source_thread, a);
        assert_eq!(web.quote_edges[0].target_thread, a);
    }

    #[test]
    fn normalize_drops_orphan_edges() {
        let mut web = empty_web();
        let a = "at://a/app.bsky.feed.post/1";
        web.add_thread(single_thread(a));
        web.quote_edges = vec![QuoteEdge {
            source: a.into(),
            target: "at://gone/app.bsky.feed.post/999".into(),
            source_thread: a.into(),
            target_thread: "at://gone/app.bsky.feed.post/999".into(),
        }];
        web.normalize_quote_edges();
        assert!(web.quote_edges.is_empty());
    }

    #[test]
    fn edge_count_includes_replies_and_quotes() {
        let mut web = empty_web();
        let root = "at://x/app.bsky.feed.post/1";
        let r = "at://x/app.bsky.feed.post/2";
        let mut t = single_thread(root);
        t.posts.insert(r.into(), reply(r, root));
        web.add_thread(t);
        web.quote_edges = vec![QuoteEdge {
            source: root.into(),
            target: "at://y/app.bsky.feed.post/3".into(),
            source_thread: root.into(),
            target_thread: "at://y/app.bsky.feed.post/3".into(),
        }];
        assert_eq!(web.edge_count(), 2);
    }

    #[test]
    fn serialization_normalizes_without_mutating() {
        let mut web = empty_web();
        let a = "at://a/app.bsky.feed.post/1";
        web.add_thread(single_thread(a));
        let orphan = QuoteEdge {
            source: a.into(),
            target: "at://gone/app.bsky.feed.post/9".into(),
            source_thread: a.into(),
            target_thread: "at://gone/app.bsky.feed.post/9".into(),
        };
        web.quote_edges = vec![orphan];
        let json = serde_json::to_value(&web).unwrap();
        assert_eq!(json["quote_edges"].as_array().unwrap().len(), 0);
        assert_eq!(json["meta"]["edge_count"], 0);
        assert_eq!(web.quote_edges.len(), 1);
    }

    #[test]
    fn web_id_is_deterministic_and_distinct() {
        let uri = "at://did:plc:test/app.bsky.feed.post/abc123";
        assert_eq!(web_id(uri), web_id(uri));
        assert!(web_id(uri).starts_with("abc123-"));
        assert_ne!(
            web_id("at://a/app.bsky.feed.post/x"),
            web_id("at://b/app.bsky.feed.post/x")
        );
    }

    #[test]
    fn web_id_matches_python() {
        let uri = "at://did:plc:test/app.bsky.feed.post/abc123";
        let digest = Sha256::digest(uri.as_bytes());
        let expected = format!("abc123-{:02x}{:02x}{:02x}", digest[0], digest[1], digest[2]);
        assert_eq!(web_id(uri), expected);
    }
}
