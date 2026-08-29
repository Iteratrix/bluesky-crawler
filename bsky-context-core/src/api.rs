//! The Bluesky `AppView` boundary: wire types and the I/O traits.
//!
//! The crawler needs exactly two XRPC calls, `app.bsky.feed.getPostThread`
//! and `app.bsky.feed.getQuotes`, both served unauthenticated by
//! `https://public.api.bsky.app`. Adapters implement [`Fetch`] over their
//! platform's HTTP client and [`Clock`] over its timers; the core never
//! touches either directly.

use core::future::Future;
use core::time::Duration;

use serde::Deserialize;

use crate::model::{Author, ByteRange, Facet, FacetFeature, Post};

/// Base URL of the public `AppView`.
pub const PUBLIC_APPVIEW: &str = "https://public.api.bsky.app";

/// Depth and parent height requested for every `getPostThread` call.
pub const THREAD_DEPTH: u32 = 1000;

/// Page size requested for every `getQuotes` call.
pub const QUOTES_PAGE_SIZE: u32 = 100;

/// Why an API call failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FetchError {
    /// The server answered with a non-success status.
    #[error("HTTP {status}: {message}")]
    Status {
        /// The HTTP status code.
        status: u16,
        /// The response body or reason phrase.
        message: String,
        /// The parsed `Retry-After` header, when present.
        retry_after: Option<Duration>,
    },
    /// The request never completed at the transport level.
    #[error("network error: {0}")]
    Network(String),
    /// The request exceeded the adapter's timeout.
    #[error("request timed out")]
    Timeout,
    /// The response body could not be decoded.
    #[error("invalid response: {0}")]
    Decode(String),
}

impl FetchError {
    /// Returns whether this is an HTTP 429 rate-limit response.
    #[must_use]
    pub fn is_rate_limited(&self) -> bool {
        match self {
            Self::Status { status, .. } => *status == 429,
            Self::Network(_) | Self::Timeout | Self::Decode(_) => false,
        }
    }
}

/// HTTP access to the two `AppView` endpoints the crawler uses.
///
/// Implementations build the XRPC query URL, perform a GET, and decode the
/// JSON body into the wire types here. Futures need not be `Send`; the
/// crawler runs on a single task.
pub trait Fetch {
    /// Calls `app.bsky.feed.getPostThread`.
    fn get_post_thread(
        &self,
        uri: &str,
        depth: u32,
        parent_height: u32,
    ) -> impl Future<Output = Result<ThreadResponse, FetchError>>;

    /// Calls `app.bsky.feed.getQuotes`, one page at a time.
    fn get_quotes(
        &self,
        uri: &str,
        limit: u32,
        cursor: Option<&str>,
    ) -> impl Future<Output = Result<QuotesResponse, FetchError>>;
}

/// Time for the crawler: deadlines, backoff, and the `crawled_at` stamp.
pub trait Clock {
    /// Returns time elapsed on a monotonic clock since an arbitrary origin.
    fn elapsed(&self) -> Duration;

    /// Returns the current wall-clock time as an RFC 3339 string.
    fn now_rfc3339(&self) -> String;

    /// Pauses for the given duration.
    fn sleep(&self, duration: Duration) -> impl Future<Output = ()>;
}

/// Response of `app.bsky.feed.getPostThread`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ThreadResponse {
    /// The requested post with its ancestors and descendants.
    pub thread: ThreadNode,
}

/// One node in a thread view: a post, or a placeholder for one that could
/// not be shown.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "$type")]
pub enum ThreadNode {
    /// A visible post with its neighbors.
    #[serde(rename = "app.bsky.feed.defs#threadViewPost")]
    Post(Box<ThreadViewPost>),
    /// The post was deleted or never existed.
    #[serde(rename = "app.bsky.feed.defs#notFoundPost")]
    NotFound {
        /// The URI that was requested.
        uri: String,
    },
    /// The post is hidden by a block.
    #[serde(rename = "app.bsky.feed.defs#blockedPost")]
    Blocked {
        /// The URI that was requested.
        uri: String,
    },
}

impl ThreadNode {
    /// Returns the post view if this node is a visible post.
    #[must_use]
    pub fn as_post(&self) -> Option<&ThreadViewPost> {
        match self {
            Self::Post(view) => Some(view),
            Self::NotFound { .. } | Self::Blocked { .. } => None,
        }
    }
}

/// A visible post together with its parent chain and replies.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ThreadViewPost {
    /// The post itself.
    pub post: PostView,
    /// The post this one replies to, when requested and present.
    #[serde(default)]
    pub parent: Option<Box<ThreadNode>>,
    /// Direct replies, when requested.
    #[serde(default)]
    pub replies: Option<Vec<ThreadNode>>,
}

/// Response of `app.bsky.feed.getQuotes`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct QuotesResponse {
    /// Posts quoting the requested URI, one page.
    #[serde(default)]
    pub posts: Vec<PostView>,
    /// Cursor for the next page, absent on the last page.
    #[serde(default)]
    pub cursor: Option<String>,
}

/// A hydrated post as the `AppView` presents it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostView {
    /// Canonical AT URI.
    pub uri: String,
    /// Content identifier.
    pub cid: String,
    /// The author's basic profile.
    pub author: ProfileViewBasic,
    /// The post record as written to the author's repository.
    pub record: PostRecord,
    /// Moderation labels applied to the post.
    #[serde(default)]
    pub labels: Option<Vec<Label>>,
    /// Reply count.
    #[serde(default)]
    pub reply_count: Option<u32>,
    /// Repost count.
    #[serde(default)]
    pub repost_count: Option<u32>,
    /// Like count.
    #[serde(default)]
    pub like_count: Option<u32>,
    /// Quote count.
    #[serde(default)]
    pub quote_count: Option<u32>,
}

/// The subset of a profile that posts carry inline.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileViewBasic {
    /// The account's DID.
    pub did: String,
    /// The account's handle.
    pub handle: String,
    /// The display name, when set.
    #[serde(default)]
    pub display_name: Option<String>,
}

/// A moderation label.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Label {
    /// The label value, e.g. `porn` or `!hide`.
    pub val: String,
}

/// An `app.bsky.feed.post` record.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostRecord {
    /// Post text.
    #[serde(default)]
    pub text: String,
    /// Creation time as written by the client.
    #[serde(default)]
    pub created_at: String,
    /// Reply references, when the post is a reply.
    #[serde(default)]
    pub reply: Option<ReplyRef>,
    /// The embed, when present.
    #[serde(default)]
    pub embed: Option<RecordEmbed>,
    /// Rich-text facets.
    #[serde(default)]
    pub facets: Option<Vec<WireFacet>>,
    /// Declared languages.
    #[serde(default)]
    pub langs: Option<Vec<String>>,
}

/// The parent and root references of a reply.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ReplyRef {
    /// The thread root.
    pub root: StrongRef,
    /// The direct parent.
    pub parent: StrongRef,
}

/// A reference to a record by URI (the `cid` is ignored).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct StrongRef {
    /// The referenced record's AT URI.
    pub uri: String,
}

/// An embed inside a post record, reduced to what the crawler cares about:
/// whether it quotes another post.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "$type")]
pub enum RecordEmbed {
    /// A quote post.
    #[serde(rename = "app.bsky.embed.record")]
    Record {
        /// The quoted record.
        record: StrongRef,
    },
    /// A quote post with attached media.
    #[serde(rename = "app.bsky.embed.recordWithMedia")]
    RecordWithMedia {
        /// The nested record embed.
        record: NestedRecordEmbed,
    },
    /// Images, external links, video, or anything else.
    #[serde(other)]
    Other,
}

/// The inner `record` of a `recordWithMedia` embed.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct NestedRecordEmbed {
    /// The quoted record.
    pub record: StrongRef,
}

impl RecordEmbed {
    /// Returns the embed's `$type` NSID and the quoted URI, if it quotes a post.
    #[must_use]
    pub fn quoted(&self) -> Option<(&'static str, &str)> {
        match self {
            Self::Record { record } => Some(("app.bsky.embed.record", &record.uri)),
            Self::RecordWithMedia { record } => {
                Some(("app.bsky.embed.recordWithMedia", &record.record.uri))
            }
            Self::Other => None,
        }
    }
}

/// A facet as written in the record.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct WireFacet {
    /// The annotated byte span.
    pub index: ByteRange,
    /// The features applied to the span.
    #[serde(default)]
    pub features: Vec<WireFeature>,
}

/// A facet feature as written in the record.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct WireFeature {
    /// The feature's `$type` NSID.
    #[serde(rename = "$type")]
    pub kind: String,
    /// Mentioned DID, for mention features.
    #[serde(default)]
    pub did: Option<String>,
    /// Link target, for link features.
    #[serde(default)]
    pub uri: Option<String>,
    /// Tag text, for tag features.
    #[serde(default)]
    pub tag: Option<String>,
}

impl From<WireFeature> for FacetFeature {
    fn from(feature: WireFeature) -> Self {
        let WireFeature {
            kind,
            did,
            uri,
            tag,
        } = feature;
        match (kind.as_str(), did, uri, tag) {
            ("app.bsky.richtext.facet#mention", Some(did), _, _) => Self::Mention { did },
            ("app.bsky.richtext.facet#link", _, Some(uri), _) => Self::Link { uri },
            ("app.bsky.richtext.facet#tag", _, _, Some(tag)) => Self::Tag { tag },
            _ => Self::Other { kind },
        }
    }
}

impl From<WireFacet> for Facet {
    fn from(facet: WireFacet) -> Self {
        let WireFacet { index, features } = facet;
        Self {
            index,
            features: features.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<PostView> for Post {
    fn from(view: PostView) -> Self {
        let PostView {
            uri,
            cid,
            author,
            record,
            labels,
            reply_count,
            repost_count,
            like_count,
            quote_count,
        } = view;
        let ProfileViewBasic {
            did,
            handle,
            display_name,
        } = author;
        let PostRecord {
            text,
            created_at,
            reply,
            embed,
            facets,
            langs,
        } = record;
        let (reply_parent, reply_root) = match reply {
            Some(ReplyRef { root, parent }) => (Some(parent.uri), Some(root.uri)),
            None => (None, None),
        };
        let (embed_type, embed_uri) = match embed.as_ref().and_then(RecordEmbed::quoted) {
            Some((kind, quoted)) => (Some(kind.to_owned()), Some(quoted.to_owned())),
            None => (None, None),
        };
        Self {
            uri,
            cid,
            author: Author {
                did,
                handle,
                display_name: display_name.unwrap_or_default(),
            },
            text,
            created_at,
            reply_parent,
            reply_root,
            embed_type,
            embed_uri,
            facets: facets
                .unwrap_or_default()
                .into_iter()
                .map(Into::into)
                .collect(),
            labels: labels
                .unwrap_or_default()
                .into_iter()
                .map(|l| l.val)
                .collect(),
            langs: langs.unwrap_or_default(),
            like_count: like_count.unwrap_or_default(),
            reply_count: reply_count.unwrap_or_default(),
            repost_count: repost_count.unwrap_or_default(),
            quote_count: quote_count.unwrap_or_default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const THREAD_JSON: &str = r#"{
      "thread": {
        "$type": "app.bsky.feed.defs#threadViewPost",
        "post": {
          "uri": "at://did:plc:root/app.bsky.feed.post/3aaa",
          "cid": "bafyroot",
          "author": {"did": "did:plc:root", "handle": "root.test", "displayName": "Root", "avatar": "https://x/y.jpg", "labels": []},
          "record": {
            "$type": "app.bsky.feed.post",
            "createdAt": "2026-08-01T10:00:00.000Z",
            "embed": {"$type": "app.bsky.embed.images", "images": [{"alt": "pic", "image": {}}]},
            "langs": ["en"],
            "text": "hello #rust"
          },
          "embed": {"$type": "app.bsky.embed.images#view", "images": []},
          "bookmarkCount": 0, "replyCount": 2, "repostCount": 1, "likeCount": 5, "quoteCount": 1,
          "indexedAt": "2026-08-01T10:00:01.000Z",
          "labels": [{"src": "did:plc:mod", "uri": "at://did:plc:root/app.bsky.feed.post/3aaa", "val": "spoiler", "cts": "2026-08-01T10:00:02.000Z"}]
        },
        "replies": [
          {
            "$type": "app.bsky.feed.defs#threadViewPost",
            "post": {
              "uri": "at://did:plc:bob/app.bsky.feed.post/3bbb",
              "cid": "bafybob",
              "author": {"did": "did:plc:bob", "handle": "bob.test"},
              "record": {
                "$type": "app.bsky.feed.post",
                "createdAt": "2026-08-01T10:05:00.000Z",
                "reply": {
                  "parent": {"cid": "bafyroot", "uri": "at://did:plc:root/app.bsky.feed.post/3aaa"},
                  "root": {"cid": "bafyroot", "uri": "at://did:plc:root/app.bsky.feed.post/3aaa"}
                },
                "facets": [{"index": {"byteStart": 0, "byteEnd": 5}, "features": [{"$type": "app.bsky.richtext.facet#mention", "did": "did:plc:root"}]},
                           {"index": {"byteStart": 6, "byteEnd": 20}, "features": [{"$type": "app.bsky.richtext.facet#link", "uri": "https://bsky.app/profile/carol.test/post/3ccc"}]},
                           {"index": {"byteStart": 21, "byteEnd": 25}, "features": [{"$type": "app.bsky.richtext.facet#weird", "foo": 1}]}],
                "text": "@root https://bsky.app/... nice"
              },
              "replyCount": 0, "repostCount": 0, "likeCount": 0, "quoteCount": 0,
              "indexedAt": "2026-08-01T10:05:01.000Z",
              "labels": []
            },
            "replies": []
          },
          {"$type": "app.bsky.feed.defs#notFoundPost", "uri": "at://did:plc:gone/app.bsky.feed.post/3ddd", "notFound": true},
          {"$type": "app.bsky.feed.defs#blockedPost", "uri": "at://did:plc:blk/app.bsky.feed.post/3eee", "blocked": true, "author": {"did": "did:plc:blk", "viewer": {"blockedBy": false}}}
        ],
        "threadContext": {}
      }
    }"#;

    const QUOTES_JSON: &str = r#"{
      "uri": "at://did:plc:root/app.bsky.feed.post/3aaa",
      "cursor": "next-page",
      "posts": [
        {
          "uri": "at://did:plc:carol/app.bsky.feed.post/3fff",
          "cid": "bafycarol",
          "author": {"did": "did:plc:carol", "handle": "carol.test", "displayName": "Carol"},
          "record": {
            "$type": "app.bsky.feed.post",
            "createdAt": "2026-08-01T11:00:00.000Z",
            "embed": {"$type": "app.bsky.embed.record", "record": {"cid": "bafyroot", "uri": "at://did:plc:root/app.bsky.feed.post/3aaa"}},
            "text": "quoting",
            "facets": null
          },
          "replyCount": 0, "repostCount": 0, "likeCount": 2, "quoteCount": 0,
          "indexedAt": "2026-08-01T11:00:01.000Z"
        },
        {
          "uri": "at://did:plc:dan/app.bsky.feed.post/3ggg",
          "cid": "bafydan",
          "author": {"did": "did:plc:dan", "handle": "dan.test"},
          "record": {
            "$type": "app.bsky.feed.post",
            "createdAt": "2026-08-01T11:30:00.000Z",
            "embed": {
              "$type": "app.bsky.embed.recordWithMedia",
              "media": {"$type": "app.bsky.embed.images", "images": []},
              "record": {"$type": "app.bsky.embed.record", "record": {"cid": "bafyroot", "uri": "at://did:plc:root/app.bsky.feed.post/3aaa"}}
            },
            "text": "quote with pic"
          },
          "indexedAt": "2026-08-01T11:30:01.000Z"
        }
      ]
    }"#;

    #[test]
    fn decodes_thread_response() {
        let resp: ThreadResponse = serde_json::from_str(THREAD_JSON).unwrap();
        let root = resp.thread.as_post().unwrap();
        assert_eq!(root.post.uri, "at://did:plc:root/app.bsky.feed.post/3aaa");
        assert!(root.parent.is_none());
        let replies = root.replies.as_ref().unwrap();
        assert_eq!(replies.len(), 3);
        assert!(replies[0].as_post().is_some());
        assert_eq!(
            replies[1],
            ThreadNode::NotFound {
                uri: "at://did:plc:gone/app.bsky.feed.post/3ddd".into()
            }
        );
        assert_eq!(
            replies[2],
            ThreadNode::Blocked {
                uri: "at://did:plc:blk/app.bsky.feed.post/3eee".into()
            }
        );
    }

    #[test]
    fn converts_root_post_view() {
        let resp: ThreadResponse = serde_json::from_str(THREAD_JSON).unwrap();
        let post = Post::from(resp.thread.as_post().unwrap().post.clone());
        assert_eq!(post.author.display_name, "Root");
        assert_eq!(post.text, "hello #rust");
        assert_eq!(post.created_at, "2026-08-01T10:00:00.000Z");
        assert_eq!(post.like_count, 5);
        assert_eq!(post.reply_count, 2);
        assert_eq!(post.repost_count, 1);
        assert_eq!(post.quote_count, 1);
        assert_eq!(post.labels, vec!["spoiler"]);
        assert_eq!(post.langs, vec!["en"]);
        assert!(post.embed_uri.is_none(), "image embeds are not quotes");
        assert!(post.embed_type.is_none());
        assert!(post.reply_parent.is_none());
    }

    #[test]
    fn converts_reply_with_facets() {
        let resp: ThreadResponse = serde_json::from_str(THREAD_JSON).unwrap();
        let replies = resp.thread.as_post().unwrap().replies.clone().unwrap();
        let post = Post::from(replies[0].as_post().unwrap().post.clone());
        assert_eq!(post.author.display_name, "");
        assert_eq!(
            post.reply_parent.as_deref(),
            Some("at://did:plc:root/app.bsky.feed.post/3aaa")
        );
        assert_eq!(post.reply_root, post.reply_parent);
        assert_eq!(post.facets.len(), 3);
        assert_eq!(
            post.facets[0].features[0],
            FacetFeature::Mention {
                did: "did:plc:root".into()
            }
        );
        assert_eq!(
            post.facets[1].features[0],
            FacetFeature::Link {
                uri: "https://bsky.app/profile/carol.test/post/3ccc".into()
            }
        );
        assert_eq!(
            post.facets[2].features[0],
            FacetFeature::Other {
                kind: "app.bsky.richtext.facet#weird".into()
            }
        );
        assert_eq!(post.facets[1].index.byte_end, 20);
        assert!(post.labels.is_empty());
    }

    #[test]
    fn decodes_quotes_including_record_with_media() {
        let resp: QuotesResponse = serde_json::from_str(QUOTES_JSON).unwrap();
        assert_eq!(resp.cursor.as_deref(), Some("next-page"));
        assert_eq!(resp.posts.len(), 2);

        let plain = Post::from(resp.posts[0].clone());
        assert_eq!(plain.embed_type.as_deref(), Some("app.bsky.embed.record"));
        assert_eq!(
            plain.embed_uri.as_deref(),
            Some("at://did:plc:root/app.bsky.feed.post/3aaa")
        );
        assert!(plain.facets.is_empty(), "null facets decode as empty");

        let with_media = Post::from(resp.posts[1].clone());
        assert_eq!(
            with_media.embed_type.as_deref(),
            Some("app.bsky.embed.recordWithMedia")
        );
        assert_eq!(
            with_media.embed_uri.as_deref(),
            Some("at://did:plc:root/app.bsky.feed.post/3aaa")
        );
        assert_eq!(with_media.like_count, 0, "missing counts default to zero");
    }

    #[test]
    fn empty_quotes_page() {
        let resp: QuotesResponse =
            serde_json::from_str(r#"{"posts": [], "uri": "at://x/app.bsky.feed.post/1"}"#).unwrap();
        assert!(resp.posts.is_empty());
        assert!(resp.cursor.is_none());
    }

    #[test]
    fn thread_with_parent_chain() {
        let json = r#"{"thread": {
          "$type": "app.bsky.feed.defs#threadViewPost",
          "post": {"uri": "at://did:plc:a/app.bsky.feed.post/2", "cid": "c2",
                   "author": {"did": "did:plc:a", "handle": "a.test"},
                   "record": {"text": "child", "createdAt": "2026-01-01T00:00:00Z",
                              "reply": {"root": {"uri": "at://did:plc:a/app.bsky.feed.post/1"}, "parent": {"uri": "at://did:plc:a/app.bsky.feed.post/1"}}}},
          "parent": {
            "$type": "app.bsky.feed.defs#threadViewPost",
            "post": {"uri": "at://did:plc:a/app.bsky.feed.post/1", "cid": "c1",
                     "author": {"did": "did:plc:a", "handle": "a.test"},
                     "record": {"text": "root", "createdAt": "2026-01-01T00:00:00Z"}}
          }
        }}"#;
        let resp: ThreadResponse = serde_json::from_str(json).unwrap();
        let node = resp.thread.as_post().unwrap();
        let parent = node.parent.as_ref().unwrap().as_post().unwrap();
        assert_eq!(parent.post.uri, "at://did:plc:a/app.bsky.feed.post/1");
        assert!(parent.replies.is_none());
    }

    #[test]
    fn rate_limit_detection() {
        let err = FetchError::Status {
            status: 429,
            message: "slow down".into(),
            retry_after: Some(Duration::from_secs(3)),
        };
        assert!(err.is_rate_limited());
        assert!(!FetchError::Timeout.is_rate_limited());
        assert_eq!(err.to_string(), "HTTP 429: slow down");
    }
}
