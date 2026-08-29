//! Pure request routing and response composition, kept free of the
//! `worker` runtime so it can be unit tested natively.

use bsky_context_core::crawler::{CrawlOptions, StopReason};
use bsky_context_core::lens::{Lens, LensKind, LensParams, NAMES, render};
use bsky_context_core::model::ContextWeb;
use bsky_context_core::uri::{PostRef, rkey_of};
use url::Url;

/// What a request asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// The usage page.
    Home,
    /// A context web.
    Web(WebQuery),
}

/// The body of a web request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebQuery {
    /// The starting post.
    pub post: PostRef,
    /// The lens to render with.
    pub lens: Lens,
    /// Whether to ignore any cached web.
    pub fresh: bool,
    /// Text or JSON.
    pub format: Format,
}

/// Response body format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// Markdown text for humans and language models.
    Markdown,
    /// The raw web JSON.
    Json,
}

/// Why a request could not be routed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RouteError {
    /// No such path.
    #[error("not found")]
    NotFound,
    /// The path or query is malformed.
    #[error("{0}")]
    BadRequest(String),
}

/// Parses a request URL.
///
/// Accepted forms:
///
/// - `/` — usage
/// - `/?url=<bsky.app URL or AT URI>`
/// - `/t/<handle or DID>/<rkey>` and `/t/<handle or DID>/<rkey>.json`
///
/// Query parameters: `lens`, `top`, `hops`, `uri`, `after`, `before`, `q`,
/// `author`, `fresh`.
///
/// # Errors
///
/// Returns [`RouteError::NotFound`] for unknown paths and
/// [`RouteError::BadRequest`] for a malformed post reference, lens, or
/// numeric parameter.
pub fn parse(url: &Url) -> Result<Route, RouteError> {
    let pairs: Vec<(String, String)> = url
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    let param = |name: &str| {
        pairs
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
            .filter(|v| !v.is_empty())
    };

    let path = url.path().trim_end_matches('/');
    let (post, format) = if path.is_empty() {
        match param("url") {
            None => return Ok(Route::Home),
            Some(target) => (parse_post(target)?, Format::Markdown),
        }
    } else {
        let Some(rest) = path.strip_prefix("/t/") else {
            return Err(RouteError::NotFound);
        };
        let (repo, rkey) = rest.split_once('/').ok_or(RouteError::NotFound)?;
        let (rkey, format) = match rkey.strip_suffix(".json") {
            Some(rkey) => (rkey, Format::Json),
            None => (rkey, Format::Markdown),
        };
        let post = parse_post(&format!("at://{repo}/app.bsky.feed.post/{rkey}"))?;
        (post, format)
    };

    let kind: LensKind = param("lens")
        .unwrap_or("tree")
        .parse()
        .map_err(|e: bsky_context_core::lens::UnknownLens| RouteError::BadRequest(e.to_string()))?;
    let number = |name: &str| -> Result<Option<usize>, RouteError> {
        param(name)
            .map(|v| {
                v.parse::<usize>().map_err(|_| {
                    RouteError::BadRequest(format!("{name} must be a non-negative integer"))
                })
            })
            .transpose()
    };
    let params = LensParams {
        top: number("top")?,
        hops: number("hops")?,
        uri: param("uri").map(str::to_owned),
        after: param("after").map(str::to_owned),
        before: param("before").map(str::to_owned),
        query: param("q").or_else(|| param("query")).map(str::to_owned),
        author: param("author").map(str::to_owned),
    };
    let fresh = match param("fresh") {
        None | Some("0" | "false" | "no") => false,
        Some(_) => true,
    };

    Ok(Route::Web(WebQuery {
        post,
        lens: kind.with_params(&params),
        fresh,
        format,
    }))
}

fn parse_post(input: &str) -> Result<PostRef, RouteError> {
    PostRef::parse(input).map_err(|e| RouteError::BadRequest(e.to_string()))
}

/// Cache key for a web reached through `post`.
///
/// Keyed by record key alone because the same post may be requested by
/// handle or by DID; [`cached_web_matches`] guards against rkey collisions
/// across repositories.
#[must_use]
pub fn cache_key(post: &PostRef) -> String {
    format!("web:{}", post.rkey)
}

/// Returns whether a cached web is really about `post`.
///
/// The root post's author must match the requested repository by handle or
/// by DID, and the root record key must match.
#[must_use]
pub fn cached_web_matches(web: &ContextWeb, post: &PostRef) -> bool {
    if rkey_of(&web.root_uri) != post.rkey {
        return false;
    }
    let Some(root) = web.get_post(&web.root_uri) else {
        return false;
    };
    root.author.did == post.repo || root.author.handle == post.repo
}

/// Usage text served at `/`.
pub const USAGE: &str = "\
# bsky-context

Fetch the full conversation graph around a Bluesky post: the reply tree and \
every quote post, recursively, rendered as text.

## Endpoints

- `/t/<handle-or-did>/<rkey>` — the web around a post, rendered through a lens
- `/t/<handle-or-did>/<rkey>.json` — the raw web as JSON
- `/?url=https://bsky.app/profile/<handle>/post/<rkey>` — same, by URL

## Query parameters

- `lens` — one of: tree (default), linear, by-author, stats, threads, highlights, \
neighborhood, timeline, search, raw
- `top` (threads, highlights), `hops` and `uri` (neighborhood), `after` and `before` \
(timeline), `q` and `author` (search)
- `fresh=1` — ignore the cached web and crawl from scratch

Crawls are bounded per request. When a response says the budget was reached, \
fetching the same URL again continues from the cached web.
";

/// Composes the markdown response for a crawled web.
#[must_use]
pub fn compose(
    web: &ContextWeb,
    lens: &Lens,
    stop_reason: StopReason,
    pending: usize,
    budget: &CrawlOptions,
) -> String {
    let root_link = web.get_post(&web.root_uri).map_or_else(
        || web.root_uri.clone(),
        |p| {
            format!(
                "https://bsky.app/profile/{}/post/{}",
                p.author.handle,
                rkey_of(&p.uri)
            )
        },
    );
    let budget_note = match stop_reason {
        StopReason::Complete => "The graph was fully explored.".to_owned(),
        StopReason::MaxNodes => format!(
            "Crawl budget reached ({} posts per request); {pending} threads unexplored. \
             Fetch this URL again to continue from the cached web.",
            budget.max_nodes
        ),
        StopReason::Timeout => format!(
            "Crawl budget reached ({}s per request); {pending} threads unexplored. \
             Fetch this URL again to continue from the cached web.",
            budget.timeout.as_secs()
        ),
    };
    format!(
        "# Bluesky context web: {root_link}\n\n\
         Crawled {}: {} posts, {} threads, {} edges. Lens: {}.\n\
         {budget_note}\n\
         Other lenses via `?lens=`: {}. Raw JSON: append `.json` to the path.\n\n---\n\n{}",
        web.crawled_at,
        web.node_count(),
        web.thread_count(),
        web.edge_count(),
        lens.kind(),
        NAMES.join(", "),
        render(web, lens)
    )
}

#[cfg(test)]
mod tests {
    use core::time::Duration;

    use bsky_context_core::model::{Author, Post, Thread};

    use super::*;

    fn parse_url(s: &str) -> Result<Route, RouteError> {
        parse(&Url::parse(s).unwrap())
    }

    fn web_query(s: &str) -> WebQuery {
        let Route::Web(q) = parse_url(s).unwrap() else {
            panic!("expected web route for {s}");
        };
        q
    }

    #[test]
    fn home_and_not_found() {
        assert_eq!(parse_url("https://x.dev/").unwrap(), Route::Home);
        assert_eq!(parse_url("https://x.dev").unwrap(), Route::Home);
        assert_eq!(
            parse_url("https://x.dev/nope").unwrap_err(),
            RouteError::NotFound
        );
        assert_eq!(
            parse_url("https://x.dev/t/onlyrepo").unwrap_err(),
            RouteError::NotFound
        );
    }

    #[test]
    fn path_form_defaults() {
        let q = web_query("https://x.dev/t/alice.bsky.social/abc123");
        assert_eq!(q.post.repo, "alice.bsky.social");
        assert_eq!(q.post.rkey, "abc123");
        assert_eq!(q.lens, Lens::Tree);
        assert!(!q.fresh);
        assert_eq!(q.format, Format::Markdown);
    }

    #[test]
    fn json_suffix_and_did() {
        let q = web_query("https://x.dev/t/did:plc:xyz/abc123.json");
        assert_eq!(q.post.repo, "did:plc:xyz");
        assert_eq!(q.post.rkey, "abc123");
        assert_eq!(q.format, Format::Json);
    }

    #[test]
    fn url_query_form() {
        let q = web_query(
            "https://x.dev/?url=https%3A%2F%2Fbsky.app%2Fprofile%2Fbob.test%2Fpost%2Fk1&lens=linear",
        );
        assert_eq!(q.post.repo, "bob.test");
        assert_eq!(q.lens, Lens::Linear);
        let q = web_query("https://x.dev/?url=at://did:plc:a/app.bsky.feed.post/k2");
        assert_eq!(q.post.rkey, "k2");
    }

    #[test]
    fn lens_params_and_fresh() {
        let q = web_query("https://x.dev/t/a.test/k?lens=search&q=hello&author=bob&fresh=1");
        assert_eq!(
            q.lens,
            Lens::Search {
                query: Some("hello".into()),
                author: Some("bob".into())
            }
        );
        assert!(q.fresh);
        let q =
            web_query("https://x.dev/t/a.test/k?lens=neighborhood&hops=1&uri=at://x&fresh=false");
        assert_eq!(
            q.lens,
            Lens::Neighborhood {
                uri: Some("at://x".into()),
                hops: 1
            }
        );
        assert!(!q.fresh);
        let q = web_query("https://x.dev/t/a.test/k?lens=threads&top=5");
        assert_eq!(q.lens, Lens::Threads { top: 5 });
        let q = web_query("https://x.dev/t/a.test/k?lens=timeline&after=2026-01-01&before=");
        assert_eq!(
            q.lens,
            Lens::Timeline {
                after: Some("2026-01-01".into()),
                before: None
            }
        );
    }

    #[test]
    fn bad_requests() {
        let err = parse_url("https://x.dev/t/a.test/k?lens=nope").unwrap_err();
        let RouteError::BadRequest(msg) = err else {
            panic!("expected BadRequest");
        };
        assert!(msg.contains("unknown lens"));
        let err = parse_url("https://x.dev/t/a.test/k?top=lots").unwrap_err();
        assert_eq!(
            err,
            RouteError::BadRequest("top must be a non-negative integer".into())
        );
        let err = parse_url("https://x.dev/?url=https://example.com").unwrap_err();
        let RouteError::BadRequest(_) = err else {
            panic!("expected BadRequest");
        };
        let err = parse_url("https://x.dev/t/a.test/not-an-rkey!").unwrap_err();
        let RouteError::BadRequest(_) = err else {
            panic!("expected BadRequest");
        };
    }

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
    fn cache_key_and_match() {
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
        assert_eq!(cache_key(&by_handle), "web:k1");
        assert_eq!(cache_key(&by_did), cache_key(&by_handle));
        assert!(cached_web_matches(&web, &by_handle));
        assert!(cached_web_matches(&web, &by_did));
        assert!(!cached_web_matches(&web, &other_repo));
        assert!(!cached_web_matches(&web, &other_rkey));
        let empty = ContextWeb::new("at://did:plc:a/app.bsky.feed.post/k1", "x");
        assert!(!cached_web_matches(&empty, &by_handle));
    }

    #[test]
    fn compose_headers() {
        let web = sample_web();
        let budget = CrawlOptions {
            max_nodes: 500,
            timeout: Duration::from_secs(20),
            ..CrawlOptions::default()
        };
        let complete = compose(&web, &Lens::Raw, StopReason::Complete, 0, &budget);
        assert!(
            complete.starts_with(
                "# Bluesky context web: https://bsky.app/profile/alice.test/post/k1\n"
            )
        );
        assert!(complete.contains("1 posts, 1 threads, 0 edges. Lens: raw."));
        assert!(complete.contains("fully explored"));
        assert!(complete.contains("\"format_version\": 2"));

        let timed_out = compose(&web, &Lens::Raw, StopReason::Timeout, 7, &budget);
        assert!(timed_out.contains("(20s per request); 7 threads unexplored"));
        let capped = compose(&web, &Lens::Raw, StopReason::MaxNodes, 3, &budget);
        assert!(capped.contains("(500 posts per request); 3 threads unexplored"));
    }

    #[test]
    fn usage_mentions_every_lens() {
        for name in NAMES {
            assert!(USAGE.contains(name), "usage lacks {name}");
        }
    }
}
