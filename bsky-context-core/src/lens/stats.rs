use core::cmp::Reverse;
use core::fmt;
use std::collections::BTreeMap;

use az::Az;
use indexmap::IndexMap;

use crate::model::{ContextWeb, Post, Thread};

fn thousands(value: impl fmt::Display) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (position, digit) in digits.chars().enumerate() {
        if position > 0 && (digits.len() - position).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

fn percent(count: usize, total: usize) -> f64 {
    if total == 0 {
        return 0.0;
    }
    count.az::<f64>() / total.az::<f64>() * 100.0
}

fn thread_size_lines(web: &ContextWeb) -> Vec<String> {
    let sizes: Vec<usize> = web.threads().values().map(Thread::post_count).collect();
    let buckets = [
        ("1 post", sizes.iter().filter(|&&size| size == 1).count()),
        (
            "2-10 posts",
            sizes
                .iter()
                .filter(|&&size| (2..=10).contains(&size))
                .count(),
        ),
        (
            "11-100 posts",
            sizes
                .iter()
                .filter(|&&size| (11..=100).contains(&size))
                .count(),
        ),
        (
            "100+ posts",
            sizes.iter().filter(|&&size| size > 100).count(),
        ),
    ];

    let mut lines = vec!["Thread sizes:".to_owned()];
    for (label, count) in buckets {
        let share = percent(count, sizes.len());
        lines.push(format!(
            "  {label:15} {} threads ({share:.1}%)",
            thousands(count)
        ));
    }
    if let Some(largest) = sizes.iter().max() {
        lines.push(format!("  Largest: {} posts", thousands(largest)));
    }
    lines
}

fn top_author_lines(web: &ContextWeb) -> Vec<String> {
    let mut counts: IndexMap<&str, (String, usize)> = IndexMap::new();
    for post in web.iter_posts() {
        let entry = counts
            .entry(post.author.handle.as_str())
            .or_insert_with(|| (super::author_name(post), 0));
        entry.1 += 1;
    }
    let mut ranked: Vec<(String, usize)> = counts.into_values().collect();
    ranked.sort_by_key(|(_, count)| Reverse(*count));

    let mut lines = vec!["Top authors by post count:".to_owned()];
    for (rank, (name, count)) in ranked.iter().take(10).enumerate() {
        lines.push(format!(
            "  {:2}. {name} - {} posts",
            rank + 1,
            thousands(count)
        ));
    }
    lines
}

fn top_post_lines(web: &ContextWeb) -> Vec<String> {
    let mut ranked: Vec<&Post> = web.iter_posts().collect();
    ranked.sort_by_key(|post| Reverse(post.engagement()));

    let mut lines = vec!["Top posts by engagement:".to_owned()];
    for (rank, post) in ranked.iter().take(10).enumerate() {
        lines.push(format!(
            "  {:2}. [{} engagement] {}",
            rank + 1,
            thousands(post.engagement()),
            super::author_name(post)
        ));
        lines.push(format!("      {}", super::truncate(&post.text, 80)));
    }
    lines
}

fn quote_hop_lines(web: &ContextWeb) -> Vec<String> {
    let Some(root_thread) = web.thread_root_for(&web.root_uri) else {
        return Vec::new();
    };
    let distances = super::thread_hop_distances(web, root_thread);

    let mut depth_posts: BTreeMap<usize, usize> = BTreeMap::new();
    for (thread_root, distance) in &distances {
        let Some(thread) = web.thread(thread_root) else {
            continue;
        };
        *depth_posts.entry(*distance).or_default() += thread.post_count();
    }
    let reachable: usize = depth_posts.values().sum();
    let unreachable = web.node_count().saturating_sub(reachable);

    let mut lines = vec!["Quote-hop depth from root thread:".to_owned()];
    for (depth, count) in &depth_posts {
        lines.push(format!("  Hop {depth}: {} posts", thousands(count)));
    }
    if unreachable > 0 {
        lines.push(format!("  Unreachable: {} posts", thousands(unreachable)));
    }
    lines
}

/// Renders summary statistics: counts, time span, thread size distribution,
/// leading authors and posts, and the quote-hop depth profile.
pub(super) fn render(web: &ContextWeb) -> String {
    let mut lines = vec!["=== CONTEXT WEB STATISTICS ===".to_owned(), String::new()];

    let reply_edges = web
        .iter_posts()
        .filter(|post| post.reply_parent.is_some())
        .count();
    let quote_edges = web.quote_edges.len();
    lines.push(format!(
        "Posts: {} across {} threads",
        thousands(web.node_count()),
        thousands(web.thread_count())
    ));
    lines.push(format!(
        "Edges: {} reply + {} quote = {} total",
        thousands(reply_edges),
        thousands(quote_edges),
        thousands(reply_edges + quote_edges)
    ));

    let times = || {
        web.iter_posts()
            .map(|post| post.created_at.as_str())
            .filter(|time| !time.is_empty())
    };
    if let (Some(earliest), Some(latest)) = (times().min(), times().max()) {
        lines.push(format!(
            "Time span: {} to {}",
            super::short_time(earliest),
            super::short_time(latest)
        ));
    }
    lines.push(String::new());

    lines.extend(thread_size_lines(web));
    lines.push(String::new());
    lines.extend(top_author_lines(web));
    lines.push(String::new());
    lines.extend(top_post_lines(web));
    lines.push(String::new());
    lines.extend(quote_hop_lines(web));

    lines.join("\n").trim_end().to_owned()
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{QUOTE, REPLY, ROOT, author, test_web};
    use super::{render, thousands};
    use crate::model::{ContextWeb, Post, QuoteEdge, Thread};

    const EXPECTED: &str = "\
=== CONTEXT WEB STATISTICS ===

Posts: 4 across 2 threads
Edges: 2 reply + 1 quote = 3 total
Time span: 2026-01-15 10:00 to 2026-01-15 10:12

Thread sizes:
  1 post          0 threads (0.0%)
  2-10 posts      2 threads (100.0%)
  11-100 posts    0 threads (0.0%)
  100+ posts      0 threads (0.0%)
  Largest: 2 posts

Top authors by post count:
   1. Bob (@bob.bsky.social) - 2 posts
   2. Alice (@alice.bsky.social) - 1 posts
   3. @carol.bsky.social - 1 posts

Top posts by engagement:
   1. [14 engagement] Alice (@alice.bsky.social)
      Original post
   2. [5 engagement] @carol.bsky.social
      Quote post
   3. [2 engagement] Bob (@bob.bsky.social)
      Direct reply
   4. [1 engagement] Bob (@bob.bsky.social)
      Reply to quote

Quote-hop depth from root thread:
  Hop 0: 2 posts
  Hop 1: 2 posts";

    #[test]
    fn matches_reference_rendering() {
        assert_eq!(render(&test_web()), EXPECTED);
    }

    #[test]
    fn counts() {
        let out = render(&test_web());
        assert!(out.contains("Posts: 4 across 2 threads"));
        assert!(out.contains("Edges: 2 reply + 1 quote = 3 total"));
    }

    #[test]
    fn top_authors() {
        let out = render(&test_web());
        assert!(out.contains("bob.bsky.social"));
        assert!(out.contains("alice.bsky.social"));
    }

    #[test]
    fn time_span() {
        let out = render(&test_web());
        assert!(out.contains("2026-01-15 10:00"));
        assert!(out.contains("2026-01-15 10:12"));
    }

    #[test]
    fn thread_sizes() {
        assert!(render(&test_web()).contains("Thread sizes:"));
    }

    #[test]
    fn top_engagement() {
        assert!(render(&test_web()).contains("Original post"));
    }

    #[test]
    fn empty_web_reports_zeroes() {
        let web = ContextWeb::new(ROOT, "2026-01-01T00:00:00Z");
        assert_eq!(
            render(&web),
            "\
=== CONTEXT WEB STATISTICS ===

Posts: 0 across 0 threads
Edges: 0 reply + 0 quote = 0 total

Thread sizes:
  1 post          0 threads (0.0%)
  2-10 posts      0 threads (0.0%)
  11-100 posts    0 threads (0.0%)
  100+ posts      0 threads (0.0%)

Top authors by post count:

Top posts by engagement:"
        );
    }

    #[test]
    fn thousands_groups_digits() {
        assert_eq!(thousands(0_usize), "0");
        assert_eq!(thousands(999_usize), "999");
        assert_eq!(thousands(1_000_usize), "1,000");
        assert_eq!(thousands(1_234_567_usize), "1,234,567");
    }

    #[test]
    fn unreachable_threads_are_counted() {
        let mut web = test_web();
        let orphan = "at://did:plc:d/app.bsky.feed.post/9";
        let mut thread = Thread::new(orphan);
        thread.posts.insert(
            orphan.into(),
            Post::new(
                orphan,
                "c9",
                author("did:plc:d", "dave.bsky.social", "Dave"),
                "Orphan post",
                "2026-01-15T09:00:00Z",
            ),
        );
        web.add_thread(thread);

        let out = render(&web);
        assert!(out.contains("Posts: 5 across 3 threads"));
        assert!(out.contains("Time span: 2026-01-15 09:00 to 2026-01-15 10:12"));
        assert!(out.contains("  1 post          1 threads (33.3%)"));
        assert!(out.contains("  2-10 posts      2 threads (66.7%)"));
        assert!(out.ends_with("  Hop 0: 2 posts\n  Hop 1: 2 posts\n  Unreachable: 1 posts"));
    }

    #[test]
    fn root_outside_the_web_drops_the_hop_section() {
        let mut web = test_web();
        web.root_uri = "at://did:plc:z/app.bsky.feed.post/99".into();
        let out = render(&web);
        assert!(!out.contains("Quote-hop depth"));
        assert!(out.ends_with("      Reply to quote"));
    }

    #[test]
    fn quote_edge_to_a_missing_thread_adds_no_depth_bucket() {
        let mut web = test_web();
        web.quote_edges.push(QuoteEdge {
            source: REPLY.into(),
            target: "at://did:plc:x/app.bsky.feed.post/77".into(),
            source_thread: ROOT.into(),
            target_thread: "at://did:plc:x/app.bsky.feed.post/77".into(),
        });
        let out = render(&web);
        assert!(out.contains("Edges: 2 reply + 2 quote = 4 total"));
        assert!(out.ends_with("  Hop 0: 2 posts\n  Hop 1: 2 posts"));
    }

    #[test]
    fn ties_keep_web_order() {
        let mut web = test_web();
        for uri in [ROOT, REPLY, QUOTE] {
            let post = web.get_post_mut(uri).unwrap();
            post.like_count = 0;
            post.repost_count = 0;
            post.quote_count = 0;
        }
        let out = render(&web);
        let top: Vec<&str> = out
            .lines()
            .skip_while(|line| *line != "Top posts by engagement:")
            .skip(1)
            .take(8)
            .step_by(2)
            .collect();
        assert_eq!(
            top,
            vec![
                "   1. [1 engagement] Bob (@bob.bsky.social)",
                "   2. [0 engagement] Alice (@alice.bsky.social)",
                "   3. [0 engagement] Bob (@bob.bsky.social)",
                "   4. [0 engagement] @carol.bsky.social",
            ]
        );
    }

    #[test]
    fn long_post_text_is_truncated() {
        let mut web = test_web();
        web.get_post_mut(ROOT).unwrap().text = "x".repeat(100);
        let out = render(&web);
        let expected = format!("      {}...", "x".repeat(77));
        assert!(out.contains(&expected), "no truncated text in:\n{out}");
    }
}
