//! The KV cache envelope: a crawled web plus when it was stored.

use az::SaturatingAs;
use bsky_context_core::model::ContextWeb;
use bsky_context_core::uri::{PostRef, rkey_of};
use serde::{Deserialize, Serialize};

/// A cached web with its storage time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    /// `Date.now()` when the entry was written, in milliseconds.
    pub stored_ms: f64,
    /// The web.
    pub web: ContextWeb,
}

impl Envelope {
    /// Returns the entry's age in whole seconds at `now_ms`, never negative.
    #[must_use]
    pub fn age_secs(&self, now_ms: f64) -> u64 {
        let age = ((now_ms - self.stored_ms) / 1000.0).max(0.0);
        age.floor().saturating_as::<u64>()
    }
}

/// Cache key for a web reached through `post`.
///
/// Keyed by record key alone because the same post may be requested by
/// handle or by DID; [`matches`] guards against rkey collisions across
/// repositories.
#[must_use]
pub fn key(post: &PostRef) -> String {
    format!("web:{}", post.rkey)
}

/// Returns whether a cached web is really about `post`.
///
/// The root post's author must match the requested repository by handle or
/// by DID, and the root record key must match.
#[must_use]
pub fn matches(web: &ContextWeb, post: &PostRef) -> bool {
    if rkey_of(&web.root_uri) != post.rkey {
        return false;
    }
    let Some(root) = web.get_post(&web.root_uri) else {
        return false;
    };
    root.author.did == post.repo || root.author.handle == post.repo
}

#[cfg(test)]
mod tests {
    use bsky_context_core::model::{Author, Post, Thread};

    use super::*;

    fn sample_web() -> ContextWeb {
        let root = "at://did:plc:a/app.bsky.feed.post/k1";
        let mut web = ContextWeb::new(root, "2026-01-01T00:00:00Z");
        let mut thread = Thread::new(root);
        thread.posts.insert(
            root.into(),
            Post::new(
                root,
                "c",
                Author {
                    did: "did:plc:a".into(),
                    handle: "alice.test".into(),
                    display_name: "Alice".into(),
                },
                "hello",
                "2026-01-01T00:00:00Z",
            ),
        );
        web.add_thread(thread);
        web
    }

    #[test]
    fn envelope_roundtrips_and_ages() {
        let envelope = Envelope {
            stored_ms: 1_000_000.0,
            web: sample_web(),
        };
        let json = serde_json::to_string(&envelope).unwrap();
        let back: Envelope = serde_json::from_str(&json).unwrap();
        assert_eq!(back, envelope);
        assert_eq!(envelope.age_secs(1_000_000.0), 0);
        assert_eq!(envelope.age_secs(1_042_999.0), 42);
        assert_eq!(envelope.age_secs(0.0), 0);
    }

    #[test]
    fn legacy_bare_web_is_not_an_envelope() {
        let json = sample_web().to_json_pretty();
        assert!(serde_json::from_str::<Envelope>(&json).is_err());
    }

    #[test]
    fn key_and_match() {
        let web = sample_web();
        let by_handle = PostRef {
            repo: "alice.test".into(),
            rkey: "k1".into(),
        };
        let by_did = PostRef {
            repo: "did:plc:a".into(),
            rkey: "k1".into(),
        };
        let other_repo = PostRef {
            repo: "mallory.test".into(),
            rkey: "k1".into(),
        };
        let other_rkey = PostRef {
            repo: "alice.test".into(),
            rkey: "k2".into(),
        };
        assert_eq!(key(&by_handle), "web:k1");
        assert_eq!(key(&by_did), key(&by_handle));
        assert!(matches(&web, &by_handle));
        assert!(matches(&web, &by_did));
        assert!(!matches(&web, &other_repo));
        assert!(!matches(&web, &other_rkey));
        let empty = ContextWeb::new("at://did:plc:a/app.bsky.feed.post/k1", "x");
        assert!(!matches(&empty, &by_handle));
    }
}
