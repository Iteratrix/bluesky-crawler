//! Parsing and normalizing Bluesky post identifiers.
//!
//! Posts are addressed by AT URIs (`at://did:plc:xyz/app.bsky.feed.post/rkey`)
//! or by web URLs (`https://bsky.app/profile/handle/post/rkey`).

use core::fmt;
use core::str::FromStr;

/// The collection NSID for Bluesky posts.
pub const POST_COLLECTION: &str = "app.bsky.feed.post";

/// A reference to a Bluesky post: a repository (DID or handle) and a record key.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PostRef {
    /// The repository owner: a DID or a handle.
    pub repo: String,
    /// The record key within the post collection.
    pub rkey: String,
}

/// Error returned when a string is neither an AT URI nor a bsky.app post URL.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("cannot parse as Bluesky post reference: {0:?}")]
pub struct ParseError(pub String);

impl PostRef {
    /// Parses an AT URI or a bsky.app post URL.
    ///
    /// Surrounding whitespace is ignored.
    ///
    /// # Errors
    ///
    /// Returns [`ParseError`] when the input matches neither form.
    ///
    /// # Examples
    ///
    /// ```
    /// use bsky_context_core::uri::PostRef;
    ///
    /// let r = PostRef::parse("https://bsky.app/profile/alice.bsky.social/post/abc").unwrap();
    /// assert_eq!(r.repo, "alice.bsky.social");
    /// assert_eq!(r.at_uri(), "at://alice.bsky.social/app.bsky.feed.post/abc");
    /// ```
    pub fn parse(input: &str) -> Result<Self, ParseError> {
        let input = input.trim();
        let parsed = parse_at_uri(input).or_else(|| parse_bsky_url(input));
        parsed.ok_or_else(|| ParseError(input.to_owned()))
    }

    /// Returns the canonical AT URI for this post.
    #[must_use]
    pub fn at_uri(&self) -> String {
        let Self { repo, rkey } = self;
        format!("at://{repo}/{POST_COLLECTION}/{rkey}")
    }
}

impl FromStr for PostRef {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl fmt::Display for PostRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.at_uri())
    }
}

fn is_rkey(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric())
}

fn parse_at_uri(input: &str) -> Option<PostRef> {
    let AtUriParts {
        authority,
        collection,
        rkey,
    } = split_at_uri(input)?;
    if collection != POST_COLLECTION || !is_rkey(rkey) {
        return None;
    }
    Some(PostRef {
        repo: authority.to_owned(),
        rkey: rkey.to_owned(),
    })
}

fn parse_bsky_url(input: &str) -> Option<PostRef> {
    let rest = input
        .strip_prefix("https://")
        .or_else(|| input.strip_prefix("http://"))?;
    let rest = rest.strip_prefix("bsky.app/profile/")?;
    let mut parts = rest.split('/');
    let handle = parts.next().filter(|h| !h.is_empty())?;
    let post = parts.next()?;
    let rkey = parts.next()?;
    if post != "post" || !is_rkey(rkey) || parts.next().is_some() {
        return None;
    }
    Some(PostRef {
        repo: handle.to_owned(),
        rkey: rkey.to_owned(),
    })
}

/// The three path segments of a generic AT URI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AtUriParts<'a> {
    /// The repository: a DID or a handle.
    pub authority: &'a str,
    /// The collection NSID.
    pub collection: &'a str,
    /// The record key.
    pub rkey: &'a str,
}

/// Splits `at://authority/collection/rkey` into its parts.
///
/// Returns [`None`] unless the input has exactly those three non-empty
/// segments.
///
/// # Examples
///
/// ```
/// use bsky_context_core::uri::split_at_uri;
///
/// let parts = split_at_uri("at://did:plc:x/app.bsky.feed.post/abc").unwrap();
/// assert_eq!(parts.authority, "did:plc:x");
/// assert_eq!(parts.rkey, "abc");
/// assert!(split_at_uri("https://bsky.app/x").is_none());
/// ```
#[must_use]
pub fn split_at_uri(uri: &str) -> Option<AtUriParts<'_>> {
    let rest = uri.strip_prefix("at://")?;
    let mut parts = rest.split('/');
    let authority = parts.next().filter(|s| !s.is_empty())?;
    let collection = parts.next().filter(|s| !s.is_empty())?;
    let rkey = parts.next().filter(|s| !s.is_empty())?;
    if parts.next().is_some() {
        return None;
    }
    Some(AtUriParts {
        authority,
        collection,
        rkey,
    })
}

/// Returns whether an AT URI addresses a post record.
///
/// Embeds and links can reference feed generators, lists, and starter
/// packs; only posts take part in a conversation graph.
///
/// # Examples
///
/// ```
/// use bsky_context_core::uri::is_post_uri;
///
/// assert!(is_post_uri("at://did:plc:x/app.bsky.feed.post/abc"));
/// assert!(!is_post_uri("at://did:plc:x/app.bsky.feed.generator/for-you"));
/// assert!(!is_post_uri("https://bsky.app/profile/x/post/abc"));
/// ```
#[must_use]
pub fn is_post_uri(uri: &str) -> bool {
    split_at_uri(uri).is_some_and(|parts| parts.collection == POST_COLLECTION)
}

/// Returns the record key of an AT URI (its final path segment).
///
/// # Examples
///
/// ```
/// assert_eq!(bsky_context_core::uri::rkey_of("at://did:plc:x/app.bsky.feed.post/abc"), "abc");
/// ```
#[must_use]
pub fn rkey_of(uri: &str) -> &str {
    uri.rsplit('/').next().unwrap_or(uri)
}

/// Interprets a link as a reference to a post, if it is one.
///
/// Accepts post AT URIs and bsky.app post URLs; anything else yields
/// [`None`]. Used to detect quote-like references in link facets.
///
/// # Examples
///
/// ```
/// use bsky_context_core::uri::post_uri_from_link;
///
/// assert_eq!(
///     post_uri_from_link("https://bsky.app/profile/a.bsky.social/post/1").as_deref(),
///     Some("at://a.bsky.social/app.bsky.feed.post/1"),
/// );
/// assert!(post_uri_from_link("https://example.com").is_none());
/// ```
#[must_use]
pub fn post_uri_from_link(url: &str) -> Option<String> {
    if url.starts_with("at://") && url.contains("/app.bsky.feed.post/") {
        return Some(url.to_owned());
    }
    PostRef::parse(url).ok().map(|r| r.at_uri())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_at_uri() {
        let r = PostRef::parse("at://did:plc:abc123/app.bsky.feed.post/xyz789").unwrap();
        assert_eq!(r.repo, "did:plc:abc123");
        assert_eq!(r.rkey, "xyz789");
    }

    #[test]
    fn parses_bsky_url() {
        let r = PostRef::parse("https://bsky.app/profile/alice.bsky.social/post/abc").unwrap();
        assert_eq!(r.repo, "alice.bsky.social");
        assert_eq!(r.rkey, "abc");
    }

    #[test]
    fn parses_bsky_url_http() {
        let r = PostRef::parse("http://bsky.app/profile/alice.bsky.social/post/abc").unwrap();
        assert_eq!(r.repo, "alice.bsky.social");
    }

    #[test]
    fn at_uri_formats() {
        let r = PostRef {
            repo: "did:plc:abc".into(),
            rkey: "xyz".into(),
        };
        assert_eq!(r.at_uri(), "at://did:plc:abc/app.bsky.feed.post/xyz");
        assert_eq!(r.to_string(), r.at_uri());
    }

    #[test]
    fn roundtrips() {
        let original = "at://did:plc:abc123/app.bsky.feed.post/xyz789";
        assert_eq!(PostRef::parse(original).unwrap().at_uri(), original);
    }

    #[test]
    fn strips_whitespace() {
        let r = PostRef::parse("  at://did:plc:abc/app.bsky.feed.post/xyz  ").unwrap();
        assert_eq!(r.repo, "did:plc:abc");
    }

    #[test]
    fn rejects_invalid() {
        assert!(PostRef::parse("not a valid uri").is_err());
        assert!(PostRef::parse("").is_err());
        assert!(PostRef::parse("at://did:plc:a/app.bsky.feed.like/xyz").is_err());
        assert!(PostRef::parse("https://bsky.app/profile/a/post/xyz/extra").is_err());
        assert!(PostRef::parse("https://bsky.app/profile/a/post/").is_err());
        let err = PostRef::parse("nope").unwrap_err();
        assert!(err.to_string().contains("cannot parse"));
    }

    #[test]
    fn from_str_works() {
        let r: PostRef = "at://did:plc:a/app.bsky.feed.post/b".parse().unwrap();
        assert_eq!(r.rkey, "b");
    }

    #[test]
    fn splits_generic_at_uri() {
        let p = split_at_uri("at://alice.bsky.social/app.bsky.feed.like/k").unwrap();
        assert_eq!(p.authority, "alice.bsky.social");
        assert_eq!(p.collection, "app.bsky.feed.like");
        assert_eq!(p.rkey, "k");
        assert!(split_at_uri("at://a/b").is_none());
        assert!(split_at_uri("at://a//c").is_none());
    }

    #[test]
    fn link_resolution() {
        assert_eq!(
            post_uri_from_link("at://did:plc:a/app.bsky.feed.post/1").as_deref(),
            Some("at://did:plc:a/app.bsky.feed.post/1")
        );
        assert!(post_uri_from_link("").is_none());
        assert!(post_uri_from_link("https://bsky.app/profile/a.bsky.social").is_none());
    }
}
