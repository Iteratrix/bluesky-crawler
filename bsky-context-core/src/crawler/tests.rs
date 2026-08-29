//! Crawler tests, ported from the Python suite that specifies this crawler.

use core::time::Duration;
use std::cell::Cell;

use futures::executor::block_on;

use super::mock::{
    FakeClock, MockFetch, at_uri, make_blocked, make_link_facet, make_not_found, make_post_view,
    make_thread_view,
};
use super::{CrawlOptions, CrawlResult, Progress, RateGate, StopReason, crawl, retry};
use crate::api::{FetchError, WireFacet, WireFeature};
use crate::model::{Author, ByteRange, ContextWeb, Facet, FacetFeature, Post, QuoteEdge, Thread};

/// Runs a crawl with the default options.
fn run(fetch: &MockFetch, start_uri: &str) -> CrawlResult {
    run_with(fetch, start_uri, &CrawlOptions::default(), None)
}

/// Runs a crawl with explicit options and an optional web to merge into.
fn run_with(
    fetch: &MockFetch,
    start_uri: &str,
    options: &CrawlOptions,
    existing: Option<ContextWeb>,
) -> CrawlResult {
    let clock = FakeClock::new();
    block_on(crawl(
        fetch,
        &clock,
        start_uri,
        options,
        existing,
        &mut |_progress| {},
    ))
}

/// Builds a post as a previous crawl would have stored it.
fn stored_post(author: &str, rkey: &str, text: &str, quote_count: u32) -> Post {
    Post {
        quote_count,
        ..Post::new(
            at_uri(author, rkey),
            format!("cid-{author}-{rkey}"),
            Author {
                did: format!("did:plc:{author}"),
                handle: format!("{author}.bsky.social"),
                display_name: String::new(),
            },
            text,
            "2026-01-01T00:00:00Z",
        )
    }
}

/// Builds a quote edge as a previous crawl would have stored it.
fn stored_edge(source: &str, target: &str) -> QuoteEdge {
    QuoteEdge {
        source: source.to_owned(),
        target: target.to_owned(),
        source_thread: source.to_owned(),
        target_thread: target.to_owned(),
    }
}

/// Builds a web holding one thread of already-crawled posts.
fn stored_web(root: &str, posts: Vec<Post>) -> ContextWeb {
    let mut web = ContextWeb::new(root, "2026-01-01T00:00:00Z");
    let mut thread = Thread::new(root);
    for post in posts {
        thread.posts.insert(post.uri.clone(), post);
    }
    web.add_thread(thread);
    web
}

mod graph_shapes {
    use super::{
        CrawlOptions, MockFetch, at_uri, make_blocked, make_not_found, make_post_view,
        make_thread_view, run,
    };
    use crate::crawler::mock::Call;

    #[test]
    fn test_singleton_post() {
        let fetch = MockFetch::new();
        fetch.add_thread(
            &at_uri("alice", "1"),
            make_thread_view(make_post_view("alice", "1").build()).build(),
        );

        let result = run(&fetch, &at_uri("alice", "1"));

        assert_eq!(result.web.node_count(), 1);
        assert_eq!(result.web.thread_count(), 1);
        assert_eq!(result.web.edge_count(), 0);
        assert_eq!(result.web.root_uri, at_uri("alice", "1"));
        assert!(fetch.quote_uris().is_empty());
        assert_eq!(
            fetch.calls(),
            vec![Call::PostThread {
                uri: at_uri("alice", "1"),
                depth: 1000,
                parent_height: 1000,
            }]
        );
    }

    #[test]
    fn test_linear_reply_chain() {
        let a = make_post_view("alice", "1").build();
        let b = make_post_view("bob", "2")
            .reply(&at_uri("alice", "1"), &at_uri("alice", "1"))
            .build();
        let c = make_post_view("carol", "3")
            .reply(&at_uri("bob", "2"), &at_uri("alice", "1"))
            .build();

        let fetch = MockFetch::new();
        fetch.add_thread(
            &at_uri("alice", "1"),
            make_thread_view(a)
                .replies(vec![
                    make_thread_view(b)
                        .replies(vec![make_thread_view(c).build()])
                        .build(),
                ])
                .build(),
        );

        let result = run(&fetch, &at_uri("alice", "1"));

        assert_eq!(result.web.node_count(), 3);
        assert_eq!(result.web.thread_count(), 1);
        assert_eq!(result.web.edge_count(), 2);
        let post_b = result.web.get_post(&at_uri("bob", "2")).unwrap();
        assert_eq!(
            post_b.reply_parent.as_deref(),
            Some(at_uri("alice", "1")).as_deref()
        );
        let post_c = result.web.get_post(&at_uri("carol", "3")).unwrap();
        assert_eq!(
            post_c.reply_parent.as_deref(),
            Some(at_uri("bob", "2")).as_deref()
        );
        assert_eq!(
            post_c.reply_root.as_deref(),
            Some(at_uri("alice", "1")).as_deref()
        );
    }

    #[test]
    fn test_wide_reply_tree() {
        let root = make_post_view("alice", "1").build();
        let replies = ["bob", "carol", "dave", "eve", "frank"]
            .into_iter()
            .enumerate()
            .map(|(index, name)| {
                make_thread_view(
                    make_post_view(name, &(index + 2).to_string())
                        .reply(&at_uri("alice", "1"), &at_uri("alice", "1"))
                        .build(),
                )
                .build()
            })
            .collect();

        let fetch = MockFetch::new();
        fetch.add_thread(
            &at_uri("alice", "1"),
            make_thread_view(root).replies(replies).build(),
        );

        let result = run(&fetch, &at_uri("alice", "1"));

        assert_eq!(result.web.node_count(), 6);
        assert_eq!(result.web.thread_count(), 1);
        assert_eq!(result.web.edge_count(), 5);
    }

    #[test]
    fn test_two_threads_linked_by_quote() {
        let a = make_post_view("alice", "1").quote_count(1).build();
        let b = make_post_view("bob", "2")
            .embed(&at_uri("alice", "1"))
            .build();

        let fetch = MockFetch::new();
        fetch.add_thread(&at_uri("alice", "1"), make_thread_view(a).build());
        fetch.add_thread(&at_uri("bob", "2"), make_thread_view(b.clone()).build());
        fetch.add_quotes(&at_uri("alice", "1"), &[b]);

        let result = run(&fetch, &at_uri("alice", "1"));

        assert_eq!(result.web.node_count(), 2);
        assert_eq!(result.web.thread_count(), 2);
        assert!(!result.web.quote_edges.is_empty());
        let edge = &result.web.quote_edges[0];
        assert_eq!(edge.source, at_uri("alice", "1"));
        assert_eq!(edge.target, at_uri("bob", "2"));
    }

    #[test]
    fn test_quote_chain() {
        let a = make_post_view("alice", "1").quote_count(1).build();
        let b = make_post_view("bob", "2")
            .embed(&at_uri("alice", "1"))
            .quote_count(1)
            .build();
        let c = make_post_view("carol", "3")
            .embed(&at_uri("bob", "2"))
            .build();

        let fetch = MockFetch::new();
        fetch.add_thread(&at_uri("alice", "1"), make_thread_view(a).build());
        fetch.add_thread(&at_uri("bob", "2"), make_thread_view(b.clone()).build());
        fetch.add_thread(&at_uri("carol", "3"), make_thread_view(c.clone()).build());
        fetch.add_quotes(&at_uri("alice", "1"), &[b]);
        fetch.add_quotes(&at_uri("bob", "2"), &[c]);

        let result = run(&fetch, &at_uri("alice", "1"));

        assert_eq!(result.web.node_count(), 3);
        assert_eq!(result.web.thread_count(), 3);
        assert_eq!(result.web.quote_edges.len(), 2);
        let sources: Vec<&str> = result
            .web
            .quote_edges
            .iter()
            .map(|edge| edge.source.as_str())
            .collect();
        let targets: Vec<&str> = result
            .web
            .quote_edges
            .iter()
            .map(|edge| edge.target.as_str())
            .collect();
        assert!(sources.contains(&at_uri("alice", "1").as_str()));
        assert!(sources.contains(&at_uri("bob", "2").as_str()));
        assert!(targets.contains(&at_uri("bob", "2").as_str()));
        assert!(targets.contains(&at_uri("carol", "3").as_str()));
    }

    #[test]
    fn test_intra_thread_quote() {
        let a = make_post_view("alice", "1").quote_count(1).build();
        let b = make_post_view("bob", "2")
            .reply(&at_uri("alice", "1"), &at_uri("alice", "1"))
            .embed(&at_uri("alice", "1"))
            .build();

        let fetch = MockFetch::new();
        fetch.add_thread(
            &at_uri("alice", "1"),
            make_thread_view(a)
                .replies(vec![make_thread_view(b.clone()).build()])
                .build(),
        );
        fetch.add_quotes(&at_uri("alice", "1"), &[b]);

        let result = run(&fetch, &at_uri("alice", "1"));

        assert_eq!(result.web.thread_count(), 1);
        assert_eq!(result.web.node_count(), 2);
        assert!(!result.web.quote_edges.is_empty());
        let edge = &result.web.quote_edges[0];
        assert_eq!(edge.source_thread, edge.target_thread);
    }

    #[test]
    fn test_mid_thread_fetch() {
        let a = make_post_view("alice", "1").build();
        let b = make_post_view("bob", "2")
            .reply(&at_uri("alice", "1"), &at_uri("alice", "1"))
            .build();
        let c = make_post_view("carol", "3")
            .reply(&at_uri("bob", "2"), &at_uri("alice", "1"))
            .build();
        let d = make_post_view("dave", "4")
            .reply(&at_uri("bob", "2"), &at_uri("alice", "1"))
            .build();

        let fetch = MockFetch::new();
        fetch.add_thread(
            &at_uri("bob", "2"),
            make_thread_view(b)
                .parent(make_thread_view(a).build())
                .replies(vec![
                    make_thread_view(c).build(),
                    make_thread_view(d).build(),
                ])
                .build(),
        );

        let result = run(&fetch, &at_uri("bob", "2"));

        assert_eq!(result.web.node_count(), 4);
        assert_eq!(result.web.thread_count(), 1);
        let thread = result.web.threads().values().next().unwrap();
        assert_eq!(thread.root_uri, at_uri("alice", "1"));
    }

    #[test]
    fn test_not_found_in_parent_chain() {
        let c = make_post_view("carol", "3")
            .reply(&at_uri("missing", "2"), &at_uri("missing", "2"))
            .build();

        let fetch = MockFetch::new();
        fetch.add_thread(
            &at_uri("carol", "3"),
            make_thread_view(c)
                .parent(make_not_found(&at_uri("missing", "2")))
                .build(),
        );

        let result = run(&fetch, &at_uri("carol", "3"));

        assert_eq!(result.web.node_count(), 1);
        assert!(result.web.has_post(&at_uri("carol", "3")));
    }

    #[test]
    fn test_blocked_post_in_replies() {
        let root = make_post_view("alice", "1").build();
        let good = make_post_view("bob", "2")
            .reply(&at_uri("alice", "1"), &at_uri("alice", "1"))
            .build();

        let fetch = MockFetch::new();
        fetch.add_thread(
            &at_uri("alice", "1"),
            make_thread_view(root)
                .replies(vec![
                    make_blocked(&at_uri("blocked", "99")),
                    make_thread_view(good).build(),
                ])
                .build(),
        );

        let result = run(&fetch, &at_uri("alice", "1"));

        assert_eq!(result.web.node_count(), 2);
    }

    #[test]
    fn test_max_depth_none_explores_everything() {
        let options = CrawlOptions::default();
        assert_eq!(options.max_depth, None);
        assert_eq!(options.max_nodes, 2000);
        assert_eq!(options.concurrency, 2);
    }
}

mod crawler_mechanics {
    use super::{
        CrawlOptions, FakeClock, MockFetch, Progress, at_uri, block_on, crawl, make_post_view,
        make_thread_view, run, run_with, stored_edge, stored_post, stored_web,
    };
    use crate::model::Post;

    #[test]
    fn test_thread_level_dedup() {
        let a = make_post_view("alice", "1").quote_count(1).build();
        let b = make_post_view("bob", "2")
            .reply(&at_uri("alice", "1"), &at_uri("alice", "1"))
            .quote_count(1)
            .build();
        let c = make_post_view("carol", "3")
            .embed(&at_uri("alice", "1"))
            .build();
        let d = make_post_view("dave", "4")
            .embed(&at_uri("bob", "2"))
            .build();

        let thread = make_thread_view(a)
            .replies(vec![make_thread_view(b).build()])
            .build();

        let fetch = MockFetch::new();
        fetch.add_thread(&at_uri("alice", "1"), thread.clone());
        fetch.add_thread(&at_uri("bob", "2"), thread);
        fetch.add_thread(&at_uri("carol", "3"), make_thread_view(c.clone()).build());
        fetch.add_thread(&at_uri("dave", "4"), make_thread_view(d.clone()).build());
        fetch.add_quotes(&at_uri("alice", "1"), &[c]);
        fetch.add_quotes(&at_uri("bob", "2"), &[d]);

        let result = run(&fetch, &at_uri("alice", "1"));

        assert_eq!(result.web.node_count(), 4);
        let thread_uris = fetch.thread_uris();
        assert!(thread_uris.contains(&at_uri("alice", "1")));
        assert!(!thread_uris.contains(&at_uri("bob", "2")));
    }

    #[test]
    fn test_max_depth() {
        let a = make_post_view("alice", "1").quote_count(1).build();
        let b = make_post_view("bob", "2")
            .embed(&at_uri("alice", "1"))
            .quote_count(1)
            .build();
        let c = make_post_view("carol", "3")
            .embed(&at_uri("bob", "2"))
            .quote_count(1)
            .build();
        let d = make_post_view("dave", "4")
            .embed(&at_uri("carol", "3"))
            .build();

        let fetch = MockFetch::new();
        fetch.add_thread(&at_uri("alice", "1"), make_thread_view(a).build());
        fetch.add_thread(&at_uri("bob", "2"), make_thread_view(b.clone()).build());
        fetch.add_thread(&at_uri("carol", "3"), make_thread_view(c.clone()).build());
        fetch.add_thread(&at_uri("dave", "4"), make_thread_view(d.clone()).build());
        fetch.add_quotes(&at_uri("alice", "1"), &[b]);
        fetch.add_quotes(&at_uri("bob", "2"), &[c]);
        fetch.add_quotes(&at_uri("carol", "3"), &[d]);

        let options = CrawlOptions {
            max_depth: Some(1),
            ..CrawlOptions::default()
        };
        let result = run_with(&fetch, &at_uri("alice", "1"), &options, None);

        assert_eq!(result.web.node_count(), 3);
        assert!(result.web.has_post(&at_uri("alice", "1")));
        assert!(result.web.has_post(&at_uri("bob", "2")));
        assert!(result.web.has_post(&at_uri("carol", "3")));
        assert!(!result.web.has_post(&at_uri("dave", "4")));
    }

    #[test]
    fn test_max_depth_zero() {
        let a = make_post_view("alice", "1").quote_count(1).build();
        let b = make_post_view("bob", "2")
            .embed(&at_uri("alice", "1"))
            .build();

        let fetch = MockFetch::new();
        fetch.add_thread(&at_uri("alice", "1"), make_thread_view(a).build());
        fetch.add_thread(&at_uri("bob", "2"), make_thread_view(b.clone()).build());
        fetch.add_quotes(&at_uri("alice", "1"), &[b]);

        let options = CrawlOptions {
            max_depth: Some(0),
            ..CrawlOptions::default()
        };
        let result = run_with(&fetch, &at_uri("alice", "1"), &options, None);

        assert_eq!(result.web.node_count(), 2);
    }

    #[test]
    fn test_max_nodes() {
        let fetch = MockFetch::new();
        let posts: Vec<_> = (0..10)
            .map(|index: usize| {
                let author = format!("user{index}");
                let mut builder = make_post_view(&author, &index.to_string());
                if index > 0 {
                    let previous = at_uri(&format!("user{}", index - 1), &(index - 1).to_string());
                    builder = builder.embed(&previous);
                }
                if index < 9 {
                    builder = builder.quote_count(1);
                }
                builder.build()
            })
            .collect();
        for (index, post) in posts.iter().enumerate() {
            let uri = at_uri(&format!("user{index}"), &index.to_string());
            fetch.add_thread(&uri, make_thread_view(post.clone()).build());
            if index < 9 {
                fetch.add_quotes(&uri, &[posts[index + 1].clone()]);
            }
        }

        let options = CrawlOptions {
            max_nodes: 3,
            ..CrawlOptions::default()
        };
        let result = run_with(&fetch, &at_uri("user0", "0"), &options, None);

        assert!(result.web.node_count() >= 3);
        assert!(result.web.node_count() <= 4);
    }

    #[test]
    fn test_recrawl_resolves_handle_uris_from_existing_posts() {
        let existing = stored_web(
            &at_uri("alice", "1"),
            vec![stored_post("alice", "1", "hi", 0)],
        );
        let handle_uri = "at://alice.bsky.social/app.bsky.feed.post/1";
        let fetch = MockFetch::new();
        fetch.add_thread(
            &at_uri("alice", "1"),
            make_thread_view(make_post_view("alice", "1").build()).build(),
        );
        fetch.add_thread(
            &at_uri("bob", "2"),
            make_thread_view(make_post_view("bob", "2").embed(handle_uri).build()).build(),
        );

        let result = run_with(
            &fetch,
            &at_uri("bob", "2"),
            &CrawlOptions::default(),
            Some(existing),
        );

        assert!(
            !fetch.thread_uris().iter().any(|u| u == handle_uri),
            "handle-based URI should resolve against the stored web: {:?}",
            fetch.thread_uris()
        );
        let edge = result
            .web
            .quote_edges
            .iter()
            .find(|e| e.target == at_uri("bob", "2"))
            .expect("quote edge");
        assert_eq!(edge.source, at_uri("alice", "1"));
    }

    #[test]
    fn test_smart_refetch_skips_unchanged_quotes() {
        let mut existing = stored_web(
            &at_uri("alice", "1"),
            vec![stored_post("alice", "1", "hi", 1)],
        );
        existing
            .quote_edges
            .push(stored_edge(&at_uri("alice", "1"), &at_uri("prev", "99")));

        let fetch = MockFetch::new();
        fetch.add_thread(
            &at_uri("alice", "1"),
            make_thread_view(make_post_view("alice", "1").quote_count(1).build()).build(),
        );
        fetch.add_quotes(&at_uri("alice", "1"), &[make_post_view("bob", "2").build()]);

        run_with(
            &fetch,
            &at_uri("alice", "1"),
            &CrawlOptions::default(),
            Some(existing),
        );

        assert!(fetch.quote_uris().is_empty());
    }

    #[test]
    fn test_smart_refetch_fetches_new_quotes() {
        let mut existing = stored_web(
            &at_uri("alice", "1"),
            vec![stored_post("alice", "1", "hi", 2)],
        );
        for index in 0..2 {
            existing.quote_edges.push(stored_edge(
                &at_uri("alice", "1"),
                &at_uri("prev", &index.to_string()),
            ));
        }

        let b = make_post_view("bob", "2")
            .embed(&at_uri("alice", "1"))
            .build();
        let fetch = MockFetch::new();
        fetch.add_thread(
            &at_uri("alice", "1"),
            make_thread_view(make_post_view("alice", "1").quote_count(5).build()).build(),
        );
        fetch.add_thread(&at_uri("bob", "2"), make_thread_view(b.clone()).build());
        fetch.add_quotes(&at_uri("alice", "1"), &[b]);

        let result = run_with(
            &fetch,
            &at_uri("alice", "1"),
            &CrawlOptions::default(),
            Some(existing),
        );

        assert!(fetch.quote_uris().contains(&at_uri("alice", "1")));
        assert_eq!(result.web.node_count(), 2);
    }

    #[test]
    fn test_smart_refetch_no_edges_means_unexplored() {
        let existing = stored_web(
            &at_uri("alice", "1"),
            vec![stored_post("alice", "1", "hi", 3)],
        );

        let b = make_post_view("bob", "2")
            .embed(&at_uri("alice", "1"))
            .build();
        let fetch = MockFetch::new();
        fetch.add_thread(
            &at_uri("alice", "1"),
            make_thread_view(make_post_view("alice", "1").quote_count(3).build()).build(),
        );
        fetch.add_thread(&at_uri("bob", "2"), make_thread_view(b.clone()).build());
        fetch.add_quotes(&at_uri("alice", "1"), &[b]);

        let result = run_with(
            &fetch,
            &at_uri("alice", "1"),
            &CrawlOptions::default(),
            Some(existing),
        );

        assert!(fetch.quote_uris().contains(&at_uri("alice", "1")));
        assert_eq!(result.web.node_count(), 2);
    }

    #[test]
    fn test_smart_refetch_mixed_explored_and_unexplored() {
        let reply = Post {
            reply_parent: Some(at_uri("alice", "1")),
            reply_root: Some(at_uri("alice", "1")),
            ..stored_post("alice", "5", "reply", 2)
        };
        let mut existing = stored_web(
            &at_uri("alice", "1"),
            vec![stored_post("alice", "1", "root", 1), reply],
        );
        existing
            .quote_edges
            .push(stored_edge(&at_uri("alice", "1"), &at_uri("prev", "99")));

        let a5 = make_post_view("alice", "5")
            .reply(&at_uri("alice", "1"), &at_uri("alice", "1"))
            .quote_count(2)
            .build();
        let b = make_post_view("bob", "2")
            .embed(&at_uri("alice", "5"))
            .build();
        let c = make_post_view("carol", "3")
            .embed(&at_uri("alice", "5"))
            .build();

        let fetch = MockFetch::new();
        fetch.add_thread(
            &at_uri("alice", "1"),
            make_thread_view(make_post_view("alice", "1").quote_count(1).build())
                .replies(vec![make_thread_view(a5).build()])
                .build(),
        );
        fetch.add_thread(&at_uri("bob", "2"), make_thread_view(b.clone()).build());
        fetch.add_thread(&at_uri("carol", "3"), make_thread_view(c.clone()).build());
        fetch.add_quotes(
            &at_uri("alice", "1"),
            &[make_post_view("skip", "99").build()],
        );
        fetch.add_quotes(&at_uri("alice", "5"), &[b, c]);

        let result = run_with(
            &fetch,
            &at_uri("alice", "1"),
            &CrawlOptions::default(),
            Some(existing),
        );

        let quote_uris = fetch.quote_uris();
        assert!(!quote_uris.contains(&at_uri("alice", "1")));
        assert!(quote_uris.contains(&at_uri("alice", "5")));
        assert!(result.web.node_count() >= 4);
    }

    #[test]
    fn test_smart_refetch_edges_exist_but_count_grew() {
        let mut existing = stored_web(
            &at_uri("alice", "1"),
            vec![stored_post("alice", "1", "hi", 1)],
        );
        existing
            .quote_edges
            .push(stored_edge(&at_uri("alice", "1"), &at_uri("prev", "99")));

        let b = make_post_view("bob", "2")
            .embed(&at_uri("alice", "1"))
            .build();
        let fetch = MockFetch::new();
        fetch.add_thread(
            &at_uri("alice", "1"),
            make_thread_view(make_post_view("alice", "1").quote_count(3).build()).build(),
        );
        fetch.add_thread(&at_uri("bob", "2"), make_thread_view(b.clone()).build());
        fetch.add_quotes(&at_uri("alice", "1"), &[b]);

        let result = run_with(
            &fetch,
            &at_uri("alice", "1"),
            &CrawlOptions::default(),
            Some(existing),
        );

        assert!(fetch.quote_uris().contains(&at_uri("alice", "1")));
        assert_eq!(result.web.node_count(), 2);
    }

    #[test]
    fn test_quote_count_zero_skips_get_quotes() {
        let a = make_post_view("alice", "1").build();
        let b = make_post_view("bob", "2")
            .reply(&at_uri("alice", "1"), &at_uri("alice", "1"))
            .build();

        let fetch = MockFetch::new();
        fetch.add_thread(
            &at_uri("alice", "1"),
            make_thread_view(a)
                .replies(vec![make_thread_view(b).build()])
                .build(),
        );

        let result = run(&fetch, &at_uri("alice", "1"));

        assert_eq!(result.web.node_count(), 2);
        assert!(fetch.quote_uris().is_empty());
    }

    #[test]
    fn test_placeholder_thread_merging() {
        let a = make_post_view("alice", "1").quote_count(1).build();
        let b = make_post_view("bob", "2").build();
        let c = make_post_view("carol", "3")
            .reply(&at_uri("bob", "2"), &at_uri("bob", "2"))
            .embed(&at_uri("alice", "1"))
            .build();

        let fetch = MockFetch::new();
        fetch.add_thread(&at_uri("alice", "1"), make_thread_view(a).build());
        fetch.add_thread(
            &at_uri("carol", "3"),
            make_thread_view(c.clone())
                .parent(make_thread_view(b.clone()).build())
                .build(),
        );
        fetch.add_thread(
            &at_uri("bob", "2"),
            make_thread_view(b)
                .replies(vec![make_thread_view(c.clone()).build()])
                .build(),
        );
        fetch.add_quotes(&at_uri("alice", "1"), &[c]);

        let result = run(&fetch, &at_uri("alice", "1"));

        assert_eq!(result.web.thread_count(), 2);
        let thread = result.web.thread(&at_uri("bob", "2")).unwrap();
        assert!(thread.posts.contains_key(&at_uri("bob", "2")));
        assert!(thread.posts.contains_key(&at_uri("carol", "3")));
    }

    #[test]
    fn test_engagement_count_updates() {
        let existing = stored_web(
            &at_uri("alice", "1"),
            vec![Post {
                like_count: 10,
                ..stored_post("alice", "1", "Original text", 0)
            }],
        );

        let fetch = MockFetch::new();
        fetch.add_thread(
            &at_uri("alice", "1"),
            make_thread_view(
                make_post_view("alice", "1")
                    .text("Different text from API")
                    .like_count(50)
                    .reply_count(8)
                    .build(),
            )
            .build(),
        );

        let result = run_with(
            &fetch,
            &at_uri("alice", "1"),
            &CrawlOptions::default(),
            Some(existing),
        );

        let post = result.web.get_post(&at_uri("alice", "1")).unwrap();
        assert_eq!(post.like_count, 50);
        assert_eq!(post.reply_count, 8);
        assert_eq!(post.text, "Original text");
    }

    #[test]
    fn test_root_uri_normalization() {
        let handle_uri = "at://alice.bsky.social/app.bsky.feed.post/1";

        let fetch = MockFetch::new();
        fetch.add_thread(
            handle_uri,
            make_thread_view(make_post_view("alice", "1").build()).build(),
        );

        let result = run(&fetch, handle_uri);

        assert_eq!(result.web.root_uri, at_uri("alice", "1"));
    }

    #[test]
    fn test_paginated_quotes() {
        let a = make_post_view("alice", "1").quote_count(3).build();
        let quoters: Vec<_> = (0..3)
            .map(|index: usize| {
                make_post_view(&format!("q{index}"), &(index + 10).to_string())
                    .embed(&at_uri("alice", "1"))
                    .build()
            })
            .collect();

        let fetch = MockFetch::new();
        fetch.add_thread(&at_uri("alice", "1"), make_thread_view(a).build());
        for quoter in &quoters {
            fetch.add_thread(&quoter.uri, make_thread_view(quoter.clone()).build());
        }
        fetch.add_quotes_paged(&at_uri("alice", "1"), &quoters, 2);

        let result = run(&fetch, &at_uri("alice", "1"));

        assert_eq!(result.web.node_count(), 4);
        assert_eq!(fetch.quote_uris().len(), 2);
    }

    #[test]
    fn test_quote_edge_dedup() {
        let a = make_post_view("alice", "1").quote_count(1).build();
        let b = make_post_view("bob", "2")
            .embed(&at_uri("alice", "1"))
            .build();

        let fetch = MockFetch::new();
        fetch.add_thread(&at_uri("alice", "1"), make_thread_view(a).build());
        fetch.add_thread(&at_uri("bob", "2"), make_thread_view(b.clone()).build());
        fetch.add_quotes(&at_uri("alice", "1"), &[b]);

        let result = run(&fetch, &at_uri("alice", "1"));

        assert_eq!(result.web.quote_edges.len(), 1);
        assert_eq!(result.web.quote_edges[0].source, at_uri("alice", "1"));
        assert_eq!(result.web.quote_edges[0].target, at_uri("bob", "2"));
    }

    #[test]
    fn test_progress_callback() {
        let fetch = MockFetch::new();
        fetch.add_thread(
            &at_uri("alice", "1"),
            make_thread_view(make_post_view("alice", "1").build()).build(),
        );

        let clock = FakeClock::new();
        let mut progress: Vec<Progress> = Vec::new();
        let result = block_on(crawl(
            &fetch,
            &clock,
            &at_uri("alice", "1"),
            &CrawlOptions::default(),
            None,
            &mut |update| progress.push(update),
        ));

        assert!(!progress.is_empty());
        let last = progress.last().unwrap();
        assert_eq!(last.node_count, result.web.node_count());
        assert_eq!(last.thread_count, result.web.thread_count());
    }
}

mod error_handling {
    use super::{
        Cell, CrawlOptions, Duration, FakeClock, FetchError, MockFetch, RateGate, StopReason,
        at_uri, block_on, make_not_found, make_post_view, make_thread_view, retry, run, run_with,
    };

    fn rate_limited() -> FetchError {
        FetchError::Status {
            status: 429,
            message: "slow down".to_owned(),
            retry_after: None,
        }
    }

    #[test]
    fn test_thread_fetch_failure() {
        let fetch = MockFetch::new();

        let result = run(&fetch, &at_uri("alice", "1"));

        assert_eq!(result.web.node_count(), 0);
    }

    #[test]
    fn test_quotes_fetch_failure() {
        let fetch = MockFetch::new();
        fetch.add_thread(
            &at_uri("alice", "1"),
            make_thread_view(make_post_view("alice", "1").quote_count(5).build()).build(),
        );
        fetch.set_quote_error(
            &at_uri("alice", "1"),
            FetchError::Status {
                status: 500,
                message: "server error".to_owned(),
                retry_after: None,
            },
        );

        let result = run(&fetch, &at_uri("alice", "1"));

        assert_eq!(result.web.node_count(), 1);
        assert_eq!(result.web.quote_edges.len(), 0);
    }

    #[test]
    fn test_retry_on_rate_limit() {
        let clock = FakeClock::new();
        let gate = RateGate::default();
        let calls = Cell::new(0_u32);

        let result: Result<&str, FetchError> = block_on(retry(&clock, &gate, || {
            let count = calls.get() + 1;
            calls.set(count);
            async move {
                if count < 3 {
                    Err(rate_limited())
                } else {
                    Ok("success")
                }
            }
        }));

        assert_eq!(result.unwrap(), "success");
        assert_eq!(calls.get(), 3);
        assert_eq!(clock.slept(), Duration::from_secs(3));
    }

    #[test]
    fn test_retry_on_network_error() {
        let clock = FakeClock::new();
        let gate = RateGate::default();
        let calls = Cell::new(0_u32);

        let result: Result<&str, FetchError> = block_on(retry(&clock, &gate, || {
            let count = calls.get() + 1;
            calls.set(count);
            async move {
                if count < 2 {
                    Err(FetchError::Network("connection reset".to_owned()))
                } else {
                    Ok("success")
                }
            }
        }));

        assert_eq!(result.unwrap(), "success");
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn test_retry_exhaustion_raises() {
        let clock = FakeClock::new();
        let gate = RateGate::default();
        let calls = Cell::new(0_u32);

        let result: Result<&str, FetchError> = block_on(retry(&clock, &gate, || {
            calls.set(calls.get() + 1);
            async {
                Err(FetchError::Status {
                    status: 500,
                    message: "permanent error".to_owned(),
                    retry_after: None,
                })
            }
        }));

        assert!(result.is_err());
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn test_retry_gives_up_on_persistent_transient_errors() {
        let clock = FakeClock::new();
        let gate = RateGate::default();
        let calls = Cell::new(0_u32);

        let result: Result<&str, FetchError> = block_on(retry(&clock, &gate, || {
            calls.set(calls.get() + 1);
            async { Err(FetchError::Timeout) }
        }));

        assert_eq!(result.unwrap_err(), FetchError::Timeout);
        assert_eq!(calls.get(), 5);
    }

    #[test]
    fn test_retry_does_not_sleep_after_the_final_rate_limit() {
        let clock = FakeClock::new();
        let gate = RateGate::default();
        let calls = Cell::new(0_u32);

        let result: Result<&str, FetchError> = block_on(retry(&clock, &gate, || {
            calls.set(calls.get() + 1);
            async {
                Err(FetchError::Status {
                    status: 429,
                    message: "slow down".to_owned(),
                    retry_after: None,
                })
            }
        }));

        assert!(result.unwrap_err().is_rate_limited());
        assert_eq!(calls.get(), 5);
        assert_eq!(clock.slept(), Duration::from_secs(1 + 2 + 4 + 8));
    }

    #[test]
    fn test_retry_honors_retry_after() {
        let clock = FakeClock::new();
        let gate = RateGate::default();
        let calls = Cell::new(0_u32);

        let result: Result<&str, FetchError> = block_on(retry(&clock, &gate, || {
            let count = calls.get() + 1;
            calls.set(count);
            async move {
                if count == 1 {
                    Err(FetchError::Status {
                        status: 429,
                        message: "slow down".to_owned(),
                        retry_after: Some(Duration::from_secs(7)),
                    })
                } else {
                    Ok("success")
                }
            }
        }));

        assert_eq!(result.unwrap(), "success");
        assert_eq!(clock.slept(), Duration::from_secs(7));
    }

    #[test]
    fn test_timeout_stops_crawl() {
        let a = make_post_view("alice", "1").quote_count(1).build();
        let b = make_post_view("bob", "2")
            .embed(&at_uri("alice", "1"))
            .quote_count(1)
            .build();
        let c = make_post_view("carol", "3")
            .embed(&at_uri("bob", "2"))
            .build();

        let fetch = MockFetch::new();
        fetch.add_thread(&at_uri("alice", "1"), make_thread_view(a).build());
        fetch.add_thread(&at_uri("bob", "2"), make_thread_view(b.clone()).build());
        fetch.add_thread(&at_uri("carol", "3"), make_thread_view(c.clone()).build());
        fetch.add_quotes(&at_uri("alice", "1"), &[b]);
        fetch.add_quotes(&at_uri("bob", "2"), &[c]);

        let options = CrawlOptions {
            timeout: Duration::ZERO,
            ..CrawlOptions::default()
        };
        let result = run_with(&fetch, &at_uri("alice", "1"), &options, None);

        assert!(result.web.node_count() < 3);
    }

    #[test]
    fn test_not_found_thread_response() {
        let fetch = MockFetch::new();
        fetch.add_thread(&at_uri("alice", "1"), make_not_found(&at_uri("alice", "1")));

        let result = run(&fetch, &at_uri("alice", "1"));

        assert_eq!(result.web.node_count(), 0);
    }

    #[test]
    fn test_termination_log_timeout() {
        let a = make_post_view("alice", "1").quote_count(1).build();
        let b = make_post_view("bob", "2")
            .embed(&at_uri("alice", "1"))
            .build();

        let fetch = MockFetch::new();
        fetch.add_thread(&at_uri("alice", "1"), make_thread_view(a).build());
        fetch.add_thread(&at_uri("bob", "2"), make_thread_view(b.clone()).build());
        fetch.add_quotes(&at_uri("alice", "1"), &[b]);

        let options = CrawlOptions {
            timeout: Duration::ZERO,
            ..CrawlOptions::default()
        };
        let result = run_with(&fetch, &at_uri("alice", "1"), &options, None);

        assert_eq!(result.stop_reason, StopReason::Timeout);
        assert!(result.pending > 0);
    }

    #[test]
    fn test_termination_log_max_nodes() {
        let a = make_post_view("alice", "1").quote_count(1).build();
        let b = make_post_view("bob", "2")
            .embed(&at_uri("alice", "1"))
            .quote_count(1)
            .build();
        let c = make_post_view("carol", "3")
            .embed(&at_uri("bob", "2"))
            .build();

        let fetch = MockFetch::new();
        fetch.add_thread(&at_uri("alice", "1"), make_thread_view(a).build());
        fetch.add_thread(&at_uri("bob", "2"), make_thread_view(b.clone()).build());
        fetch.add_thread(&at_uri("carol", "3"), make_thread_view(c.clone()).build());
        fetch.add_quotes(&at_uri("alice", "1"), &[b]);
        fetch.add_quotes(&at_uri("bob", "2"), &[c]);

        let options = CrawlOptions {
            max_nodes: 1,
            ..CrawlOptions::default()
        };
        let result = run_with(&fetch, &at_uri("alice", "1"), &options, None);

        assert_eq!(result.stop_reason, StopReason::MaxNodes);
    }

    #[test]
    fn test_termination_log_complete() {
        let fetch = MockFetch::new();
        fetch.add_thread(
            &at_uri("alice", "1"),
            make_thread_view(make_post_view("alice", "1").build()).build(),
        );

        let result = run(&fetch, &at_uri("alice", "1"));

        assert_eq!(result.stop_reason, StopReason::Complete);
        assert_eq!(result.pending, 0);
    }

    #[test]
    fn test_termination_log_max_depth_still_completes() {
        let a = make_post_view("alice", "1").quote_count(1).build();
        let b = make_post_view("bob", "2")
            .embed(&at_uri("alice", "1"))
            .quote_count(1)
            .build();
        let c = make_post_view("carol", "3")
            .embed(&at_uri("bob", "2"))
            .build();

        let fetch = MockFetch::new();
        fetch.add_thread(&at_uri("alice", "1"), make_thread_view(a).build());
        fetch.add_thread(&at_uri("bob", "2"), make_thread_view(b.clone()).build());
        fetch.add_thread(&at_uri("carol", "3"), make_thread_view(c.clone()).build());
        fetch.add_quotes(&at_uri("alice", "1"), &[b]);
        fetch.add_quotes(&at_uri("bob", "2"), &[c]);

        let options = CrawlOptions {
            max_depth: Some(0),
            ..CrawlOptions::default()
        };
        let result = run_with(&fetch, &at_uri("alice", "1"), &options, None);

        assert_eq!(result.stop_reason, StopReason::Complete);
        assert_eq!(result.pending, 0);
    }
}

mod edge_cases {
    use super::{
        CrawlOptions, MockFetch, at_uri, make_post_view, make_thread_view, run, run_with,
        stored_post, stored_web,
    };
    use crate::model::Post;

    #[test]
    fn test_heavily_quoted_post() {
        let a = make_post_view("alice", "1").quote_count(10).build();
        let quoters: Vec<_> = (0..10)
            .map(|index: usize| {
                make_post_view(&format!("q{index}"), &(index + 10).to_string())
                    .embed(&at_uri("alice", "1"))
                    .build()
            })
            .collect();

        let fetch = MockFetch::new();
        fetch.add_thread(&at_uri("alice", "1"), make_thread_view(a).build());
        for quoter in &quoters {
            fetch.add_thread(&quoter.uri, make_thread_view(quoter.clone()).build());
        }
        fetch.add_quotes(&at_uri("alice", "1"), &quoters);

        let result = run(&fetch, &at_uri("alice", "1"));

        assert_eq!(result.web.thread_count(), 11);
        assert_eq!(result.web.quote_edges.len(), 10);
    }

    #[test]
    fn test_concurrency_does_not_change_the_web() {
        let counts: Vec<(usize, usize, usize)> = [1, 2, 5]
            .into_iter()
            .map(|concurrency| {
                let a = make_post_view("alice", "1").quote_count(10).build();
                let quoters: Vec<_> = (0..10)
                    .map(|index: usize| {
                        make_post_view(&format!("q{index}"), &(index + 10).to_string())
                            .embed(&at_uri("alice", "1"))
                            .quote_count(0)
                            .build()
                    })
                    .collect();

                let fetch = MockFetch::new();
                fetch.add_thread(&at_uri("alice", "1"), make_thread_view(a).build());
                for quoter in &quoters {
                    fetch.add_thread(&quoter.uri, make_thread_view(quoter.clone()).build());
                }
                fetch.add_quotes(&at_uri("alice", "1"), &quoters);

                let options = CrawlOptions {
                    concurrency,
                    ..CrawlOptions::default()
                };
                let result = run_with(&fetch, &at_uri("alice", "1"), &options, None);
                (
                    result.web.node_count(),
                    result.web.thread_count(),
                    result.web.quote_edges.len(),
                )
            })
            .collect();

        assert_eq!(counts, vec![(11, 11, 10); 3]);
    }

    #[test]
    fn test_diamond_quote() {
        let a = make_post_view("alice", "1").quote_count(2).build();
        let b = make_post_view("bob", "2")
            .embed(&at_uri("alice", "1"))
            .build();
        let c = make_post_view("carol", "3")
            .embed(&at_uri("alice", "1"))
            .build();

        let fetch = MockFetch::new();
        fetch.add_thread(&at_uri("alice", "1"), make_thread_view(a).build());
        fetch.add_thread(&at_uri("bob", "2"), make_thread_view(b.clone()).build());
        fetch.add_thread(&at_uri("carol", "3"), make_thread_view(c.clone()).build());
        fetch.add_quotes(&at_uri("alice", "1"), &[b, c]);

        let result = run(&fetch, &at_uri("alice", "1"));

        assert_eq!(result.web.node_count(), 3);
        assert_eq!(result.web.thread_count(), 3);
        assert_eq!(result.web.quote_edges.len(), 2);
        let alice_calls = fetch
            .thread_uris()
            .into_iter()
            .filter(|uri| *uri == at_uri("alice", "1"))
            .count();
        assert_eq!(alice_calls, 1);
    }

    #[test]
    fn test_record_with_media_embed() {
        let a = make_post_view("alice", "1").build();
        let b = make_post_view("bob", "2")
            .embed_with_media(&at_uri("alice", "1"))
            .build();

        let fetch = MockFetch::new();
        fetch.add_thread(&at_uri("alice", "1"), make_thread_view(a).build());
        fetch.add_thread(&at_uri("bob", "2"), make_thread_view(b).build());

        let result = run(&fetch, &at_uri("bob", "2"));

        assert_eq!(result.web.node_count(), 2);
        assert!(!result.web.quote_edges.is_empty());
        let post_b = result.web.get_post(&at_uri("bob", "2")).unwrap();
        assert_eq!(
            post_b.embed_type.as_deref(),
            Some("app.bsky.embed.recordWithMedia")
        );
    }

    #[test]
    fn test_recrawl_preserves_and_adds_threads() {
        let reply = Post {
            reply_parent: Some(at_uri("alice", "1")),
            reply_root: Some(at_uri("alice", "1")),
            ..stored_post("bob", "2", "Reply", 0)
        };
        let existing = stored_web(
            &at_uri("alice", "1"),
            vec![stored_post("alice", "1", "Root post", 0), reply],
        );

        let b = make_post_view("bob", "2")
            .reply(&at_uri("alice", "1"), &at_uri("alice", "1"))
            .build();
        let c = make_post_view("carol", "3")
            .embed(&at_uri("alice", "1"))
            .build();

        let fetch = MockFetch::new();
        fetch.add_thread(
            &at_uri("alice", "1"),
            make_thread_view(make_post_view("alice", "1").quote_count(1).build())
                .replies(vec![make_thread_view(b).build()])
                .build(),
        );
        fetch.add_thread(&at_uri("carol", "3"), make_thread_view(c.clone()).build());
        fetch.add_quotes(&at_uri("alice", "1"), &[c]);

        let result = run_with(
            &fetch,
            &at_uri("alice", "1"),
            &CrawlOptions::default(),
            Some(existing),
        );

        assert_eq!(result.web.thread_count(), 2);
        assert!(result.web.thread(&at_uri("alice", "1")).is_some());
        assert_eq!(result.web.node_count(), 3);
        let thread = result.web.thread(&at_uri("alice", "1")).unwrap();
        assert!(thread.posts.contains_key(&at_uri("alice", "1")));
        assert!(thread.posts.contains_key(&at_uri("bob", "2")));
    }
}

mod facet_edges {
    use super::{MockFetch, at_uri, make_link_facet, make_post_view, make_thread_view, run};

    #[test]
    fn test_link_facet_creates_quote_edge() {
        let handle_uri = "at://carol.bsky.social/app.bsky.feed.post/5";
        let a = make_post_view("alice", "1")
            .facets(vec![make_link_facet(
                "https://bsky.app/profile/carol.bsky.social/post/5",
            )])
            .build();

        let fetch = MockFetch::new();
        fetch.add_thread(&at_uri("alice", "1"), make_thread_view(a).build());
        fetch.add_thread(
            handle_uri,
            make_thread_view(make_post_view("carol", "5").build()).build(),
        );

        let result = run(&fetch, &at_uri("alice", "1"));

        assert_eq!(result.web.node_count(), 2);
        assert!(!result.web.quote_edges.is_empty());
        let targets: Vec<&str> = result
            .web
            .quote_edges
            .iter()
            .map(|edge| edge.target.as_str())
            .collect();
        assert!(targets.contains(&at_uri("alice", "1").as_str()));
    }

    #[test]
    fn test_link_facet_at_uri() {
        let target_uri = at_uri("carol", "5");
        let a = make_post_view("alice", "1")
            .facets(vec![make_link_facet(&target_uri)])
            .build();

        let fetch = MockFetch::new();
        fetch.add_thread(&at_uri("alice", "1"), make_thread_view(a).build());
        fetch.add_thread(
            &target_uri,
            make_thread_view(make_post_view("carol", "5").build()).build(),
        );

        let result = run(&fetch, &at_uri("alice", "1"));

        assert_eq!(result.web.node_count(), 2);
        assert!(!result.web.quote_edges.is_empty());
    }

    #[test]
    fn test_link_facet_skips_non_post_urls() {
        let a = make_post_view("alice", "1")
            .facets(vec![make_link_facet("https://example.com/some-page")])
            .build();

        let fetch = MockFetch::new();
        fetch.add_thread(&at_uri("alice", "1"), make_thread_view(a).build());

        let result = run(&fetch, &at_uri("alice", "1"));

        assert_eq!(result.web.node_count(), 1);
        assert_eq!(result.web.quote_edges.len(), 0);
    }

    #[test]
    fn test_link_facet_deduped_with_embed() {
        let target_uri = at_uri("carol", "5");
        let a = make_post_view("alice", "1")
            .embed(&target_uri)
            .facets(vec![make_link_facet(&target_uri)])
            .build();

        let fetch = MockFetch::new();
        fetch.add_thread(&at_uri("alice", "1"), make_thread_view(a).build());
        fetch.add_thread(
            &target_uri,
            make_thread_view(make_post_view("carol", "5").build()).build(),
        );

        let result = run(&fetch, &at_uri("alice", "1"));

        assert_eq!(result.web.quote_edges.len(), 1);
    }
}

mod unknown_facets {
    use super::{ByteRange, Facet, FacetFeature, WireFacet, WireFeature};

    #[test]
    fn test_unknown_facet_type_preserved() {
        let wire = WireFacet {
            index: ByteRange {
                byte_start: 0,
                byte_end: 5,
            },
            features: vec![WireFeature {
                kind: "app.bsky.richtext.facet#futureType".to_owned(),
                did: None,
                uri: None,
                tag: None,
            }],
        };

        let facet = Facet::from(wire);

        assert_eq!(facet.features.len(), 1);
        assert_eq!(
            facet.features[0],
            FacetFeature::Other {
                kind: "app.bsky.richtext.facet#futureType".to_owned()
            }
        );
    }
}
