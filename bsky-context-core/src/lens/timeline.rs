use std::collections::HashMap;

use indexmap::IndexMap;

use crate::model::{ContextWeb, Post};

use super::threads::comma;
use super::{author_name, short_time};

/// Renders posts inside a time window, oldest first.
pub(super) fn render(web: &ContextWeb, after: Option<&str>, before: Option<&str>) -> String {
    let after = after.filter(|bound| !bound.is_empty());
    let before = before.filter(|bound| !bound.is_empty());

    let nodes = web.nodes();
    let mut posts: Vec<&Post> = nodes.values().copied().collect();
    posts.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    if let Some(after) = after {
        posts.retain(|post| post.created_at.as_str() >= after);
    }
    if let Some(before) = before {
        posts.retain(|post| post.created_at.as_str() < before);
    }

    let total = posts.len();
    let numbers: HashMap<&str, usize> = posts
        .iter()
        .enumerate()
        .map(|(offset, post)| (post.uri.as_str(), offset + 1))
        .collect();

    let window = match (after, before) {
        (Some(after), Some(before)) => {
            format!("{} to {}", short_time(after), short_time(before))
        }
        (Some(after), None) => format!("after {}", short_time(after)),
        (None, Some(before)) => format!("before {}", short_time(before)),
        (None, None) => "all time".to_owned(),
    };

    let mut lines = vec![
        format!("=== TIMELINE ({window}) ==="),
        format!("Posts: {} of {}", comma(total), comma(web.node_count())),
        String::new(),
    ];

    for (offset, post) in posts.iter().enumerate() {
        let number = offset + 1;
        let mut context: Vec<String> = Vec::new();
        context.extend(reference(
            &nodes,
            &numbers,
            "replying to",
            post.reply_parent.as_deref(),
        ));
        context.extend(reference(
            &nodes,
            &numbers,
            "quoting",
            post.embed_uri.as_deref(),
        ));
        let context = if context.is_empty() {
            String::new()
        } else {
            format!("  [{}]", context.join(", "))
        };

        lines.push(format!(
            "[{number}/{total}] {}  {}{context}",
            author_name(post),
            short_time(&post.created_at)
        ));
        for text_line in post.text.lines() {
            lines.push(format!("  {text_line}"));
        }
        lines.push(String::new());
    }

    lines.join("\n").trim_end().to_owned()
}

fn reference(
    nodes: &IndexMap<&str, &Post>,
    numbers: &HashMap<&str, usize>,
    verb: &str,
    uri: Option<&str>,
) -> Option<String> {
    let uri = uri?;
    let post = nodes.get(uri)?;
    let handle = &post.author.handle;
    Some(match numbers.get(uri) {
        Some(number) => format!("{verb} @{handle} #{number}"),
        None => format!("{verb} @{handle}"),
    })
}

#[cfg(test)]
mod tests {
    use crate::lens::fixtures::{ROOT, test_web};
    use crate::model::ContextWeb;

    use super::render;

    #[test]
    fn no_bounds_renders_every_post_in_order() {
        let out = render(&test_web(), None, None);
        assert_eq!(
            out,
            "=== TIMELINE (all time) ===\n\
             Posts: 4 of 4\n\
             \n\
             [1/4] Alice (@alice.bsky.social)  2026-01-15 10:00\n\
             \x20 Original post\n\
             \n\
             [2/4] Bob (@bob.bsky.social)  2026-01-15 10:05  [replying to @alice.bsky.social #1]\n\
             \x20 Direct reply\n\
             \n\
             [3/4] @carol.bsky.social  2026-01-15 10:08  [quoting @alice.bsky.social #1]\n\
             \x20 Quote post\n\
             \n\
             [4/4] Bob (@bob.bsky.social)  2026-01-15 10:12  [replying to @carol.bsky.social #3]\n\
             \x20 Reply to quote"
        );
    }

    #[test]
    fn after_bound_drops_earlier_posts_and_renumbers() {
        let out = render(&test_web(), Some("2026-01-15T10:06:00Z"), None);
        assert_eq!(
            out,
            "=== TIMELINE (after 2026-01-15 10:06) ===\n\
             Posts: 2 of 4\n\
             \n\
             [1/2] @carol.bsky.social  2026-01-15 10:08  [quoting @alice.bsky.social]\n\
             \x20 Quote post\n\
             \n\
             [2/2] Bob (@bob.bsky.social)  2026-01-15 10:12  [replying to @carol.bsky.social #1]\n\
             \x20 Reply to quote"
        );
        assert!(!out.contains("Original post"));
        assert!(!out.contains("Direct reply"));
    }

    #[test]
    fn before_bound_keeps_earlier_posts() {
        let out = render(&test_web(), None, Some("2026-01-15T10:06:00Z"));
        assert!(out.starts_with("=== TIMELINE (before 2026-01-15 10:06) ===\nPosts: 2 of 4"));
        assert!(out.contains("Original post"));
        assert!(out.contains("Direct reply"));
        assert!(!out.contains("Quote post"));
    }

    #[test]
    fn window_uses_both_bounds() {
        let out = render(
            &test_web(),
            Some("2026-01-15T10:04:00Z"),
            Some("2026-01-15T10:09:00Z"),
        );
        assert!(
            out.starts_with(
                "=== TIMELINE (2026-01-15 10:04 to 2026-01-15 10:09) ===\nPosts: 2 of 4"
            )
        );
        assert!(out.contains("[1/2]"));
        assert!(out.contains("[2/2]"));
        assert!(out.contains("Direct reply"));
        assert!(out.contains("Quote post"));
    }

    #[test]
    fn after_is_inclusive_and_before_is_exclusive() {
        let bound = "2026-01-15T10:05:00Z";
        let inclusive = render(&test_web(), Some(bound), None);
        assert!(inclusive.contains("Direct reply"));
        assert!(inclusive.starts_with("=== TIMELINE (after 2026-01-15 10:05) ===\nPosts: 3 of 4"));

        let exclusive = render(&test_web(), None, Some(bound));
        assert!(!exclusive.contains("Direct reply"));
        assert!(exclusive.starts_with("=== TIMELINE (before 2026-01-15 10:05) ===\nPosts: 1 of 4"));
    }

    #[test]
    fn empty_bounds_behave_like_no_bounds() {
        assert_eq!(
            render(&test_web(), Some(""), Some("")),
            render(&test_web(), None, None)
        );
    }

    #[test]
    fn window_excluding_everything_renders_the_header_only() {
        let out = render(&test_web(), Some("2027-01-01T00:00:00Z"), None);
        assert_eq!(
            out,
            "=== TIMELINE (after 2027-01-01 00:00) ===\nPosts: 0 of 4"
        );
    }

    #[test]
    fn empty_web_renders_the_header_only() {
        let web = ContextWeb::new(ROOT, "2026-01-01T00:00:00Z");
        assert_eq!(
            render(&web, None, None),
            "=== TIMELINE (all time) ===\nPosts: 0 of 0"
        );
    }
}
