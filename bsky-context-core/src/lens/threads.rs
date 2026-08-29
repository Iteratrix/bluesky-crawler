use std::cmp::Reverse;

use crate::model::{ContextWeb, Thread};

use super::{author_name, counted, thousands, truncate};

/// Formats an integer with `,` between groups of three digits.
struct ThreadInfo<'a> {
    size: usize,
    engagement: u64,
    name: String,
    text: String,
    uri: &'a str,
}

/// Lists threads sorted by post count, largest first.
pub(super) fn render(web: &ContextWeb, top: usize) -> String {
    let mut infos: Vec<ThreadInfo<'_>> = Vec::new();
    for Thread { root_uri, posts } in web.threads().values() {
        let engagement: u64 = posts
            .values()
            .map(|post| u64::from(post.engagement()))
            .sum();
        let Some(post) = posts.get(root_uri).or_else(|| posts.values().next()) else {
            continue;
        };
        infos.push(ThreadInfo {
            size: posts.len(),
            engagement,
            name: author_name(post),
            text: truncate(&post.text, 80),
            uri: post.uri.as_str(),
        });
    }

    infos.sort_by_key(|info| Reverse(info.size));

    let shown = top.min(infos.len());
    let mut lines = vec![
        format!(
            "=== THREADS ({} total, showing top {shown}) ===",
            thousands(infos.len())
        ),
        String::new(),
    ];

    for (
        offset,
        ThreadInfo {
            size,
            engagement,
            name,
            text,
            uri,
        },
    ) in infos.iter().take(top).enumerate()
    {
        let rank = offset + 1;
        lines.push(format!(
            "#{rank:<3} {} | {} engagement | {name}",
            counted(*size, "post", "posts"),
            thousands(engagement)
        ));
        lines.push(format!("     {text}"));
        lines.push(format!("     {uri}"));
        lines.push(String::new());
    }

    lines.join("\n").trim_end().to_owned()
}

#[cfg(test)]
mod tests {
    #[test]
    fn header_counts_only_threads_with_posts() {
        let mut web = super::super::fixtures::test_web();
        web.add_thread(crate::model::Thread::new(
            "at://did:plc:z/app.bsky.feed.post/9",
        ));
        let out = super::render(&web, 20);
        assert!(
            out.starts_with("=== THREADS (2 total, showing top 2) ==="),
            "{out}"
        );
    }

    use crate::lens::fixtures::{ROOT, test_web};
    use crate::model::ContextWeb;

    use super::render;

    #[test]
    fn renders_both_threads_in_size_order() {
        let out = render(&test_web(), 20);
        assert_eq!(
            out,
            "=== THREADS (2 total, showing top 2) ===\n\
             \n\
             #1   2 posts | 16 engagement | Alice (@alice.bsky.social)\n\
             \x20    Original post\n\
             \x20    at://did:plc:a/app.bsky.feed.post/1\n\
             \n\
             #2   2 posts | 6 engagement | @carol.bsky.social\n\
             \x20    Quote post\n\
             \x20    at://did:plc:c/app.bsky.feed.post/3"
        );
    }

    #[test]
    fn shows_post_counts_and_engagement() {
        let out = render(&test_web(), 20);
        assert!(out.contains("2 posts"));
        assert!(out.contains("engagement"));
        assert!(out.contains("Original post"));
    }

    #[test]
    fn top_smaller_than_thread_count_truncates() {
        let out = render(&test_web(), 1);
        assert!(out.starts_with("=== THREADS (2 total, showing top 1) ==="));
        assert!(out.contains("Original post"));
        assert!(!out.contains("Quote post"));
    }

    #[test]
    fn top_larger_than_thread_count_clamps_header() {
        let out = render(&test_web(), 99);
        assert!(out.starts_with("=== THREADS (2 total, showing top 2) ==="));
        assert_eq!(out.matches("posts |").count(), 2);
    }

    #[test]
    fn top_zero_renders_header_only() {
        let out = render(&test_web(), 0);
        assert_eq!(out, "=== THREADS (2 total, showing top 0) ===");
    }

    #[test]
    fn thread_without_its_root_post_falls_back_to_the_first_post() {
        let mut web = test_web();
        let mut thread = web.remove_thread(ROOT).unwrap();
        thread.posts.shift_remove(ROOT);
        web.add_thread(thread);
        let out = render(&web, 20);
        assert!(out.contains("#1   2 posts | 6 engagement | @carol.bsky.social"));
        assert!(out.contains("#2   1 post | 2 engagement | Bob (@bob.bsky.social)"));
        assert!(out.contains("     Direct reply"));
        assert!(out.contains("     at://did:plc:b/app.bsky.feed.post/2"));
    }

    #[test]
    fn empty_web_renders_header_only() {
        let web = ContextWeb::new(
            "at://did:plc:a/app.bsky.feed.post/1",
            "2026-01-01T00:00:00Z",
        );
        assert_eq!(render(&web, 20), "=== THREADS (0 total, showing top 0) ===");
    }
}
