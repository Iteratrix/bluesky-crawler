use indexmap::IndexMap;

use crate::model::{ContextWeb, Post, Thread};

use super::{author_name, counted, short_time, truncate};

/// Filters posts by a text query and/or an author handle substring.
pub(super) fn render(web: &ContextWeb, query: Option<&str>, author: Option<&str>) -> String {
    let query = query.filter(|value| !value.is_empty());
    let author = author.filter(|value| !value.is_empty());
    if query.is_none() && author.is_none() {
        return "No search criteria provided. Use --query and/or --author.".to_owned();
    }

    let query_lower = query.map(str::to_lowercase);
    let author_lower = author.map(str::to_lowercase);

    let nodes = web.nodes();
    let mut matches: Vec<&Post> = web.iter_posts().collect();
    matches.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    matches.retain(|post| {
        let text_matches = query_lower
            .as_ref()
            .is_none_or(|query| post.text.to_lowercase().contains(query));
        let author_matches = author_lower
            .as_ref()
            .is_none_or(|author| post.author.handle.to_lowercase().contains(author));
        text_matches && author_matches
    });

    let mut filters: Vec<String> = Vec::new();
    if let Some(query) = query {
        filters.push(format!("query: \"{query}\""));
    }
    if let Some(author) = author {
        filters.push(format!("author: {author}"));
    }

    let mut lines = vec![
        "=== SEARCH RESULTS ===".to_owned(),
        format!(
            "{} | {} in {}",
            filters.join(" | "),
            counted(matches.len(), "match", "matches"),
            counted(web.node_count(), "post", "posts")
        ),
        String::new(),
    ];

    for (offset, post) in matches.iter().enumerate() {
        let number = offset + 1;
        let thread_root = web.thread_root_for(&post.uri).unwrap_or("None");
        let thread_size = web.thread(thread_root).map_or(0, Thread::post_count);

        lines.push(format!(
            "[{number}] {}  {}",
            author_name(post),
            short_time(&post.created_at)
        ));
        lines.push(format!(
            "    Thread: {thread_root} ({})",
            counted(thread_size, "post", "posts")
        ));

        let mut context: Vec<String> = Vec::new();
        context.extend(
            handle_of(&nodes, post.reply_parent.as_deref())
                .map(|handle| format!("replying to @{handle}")),
        );
        context.extend(
            handle_of(&nodes, post.embed_uri.as_deref()).map(|handle| format!("quoting @{handle}")),
        );
        if !context.is_empty() {
            lines.push(format!("    [{}]", context.join(", ")));
        }

        lines.push(format!("    {}", truncate(&post.text, 120)));
        if post.engagement() > 0 {
            lines.push(format!(
                "    ({}, {}, {})",
                counted(post.like_count, "like", "likes"),
                counted(post.repost_count, "repost", "reposts"),
                counted(post.quote_count, "quote", "quotes")
            ));
        }
        lines.push(String::new());
    }

    lines.join("\n").trim_end().to_owned()
}

fn handle_of<'a>(nodes: &IndexMap<&str, &'a Post>, uri: Option<&str>) -> Option<&'a str> {
    let post = nodes.get(uri?)?;
    Some(post.author.handle.as_str())
}

#[cfg(test)]
mod tests {
    use crate::lens::fixtures::{ROOT, test_web};
    use crate::model::ContextWeb;

    use super::render;

    #[test]
    fn query_matches_post_text() {
        let out = render(&test_web(), Some("reply"), None);
        assert_eq!(
            out,
            "=== SEARCH RESULTS ===\n\
             query: \"reply\" | 2 matches in 4 posts\n\
             \n\
             [1] Bob (@bob.bsky.social)  2026-01-15 10:05\n\
             \x20   Thread: at://did:plc:a/app.bsky.feed.post/1 (2 posts)\n\
             \x20   [replying to @alice.bsky.social]\n\
             \x20   Direct reply\n\
             \x20   (2 likes, 0 reposts, 0 quotes)\n\
             \n\
             [2] Bob (@bob.bsky.social)  2026-01-15 10:12\n\
             \x20   Thread: at://did:plc:c/app.bsky.feed.post/3 (2 posts)\n\
             \x20   [replying to @carol.bsky.social]\n\
             \x20   Reply to quote\n\
             \x20   (1 like, 0 reposts, 0 quotes)"
        );
    }

    #[test]
    fn author_only_matches_handle_substring() {
        let out = render(&test_web(), None, Some("carol"));
        assert_eq!(
            out,
            "=== SEARCH RESULTS ===\n\
             author: carol | 1 match in 4 posts\n\
             \n\
             [1] @carol.bsky.social  2026-01-15 10:08\n\
             \x20   Thread: at://did:plc:c/app.bsky.feed.post/3 (2 posts)\n\
             \x20   [quoting @alice.bsky.social]\n\
             \x20   Quote post\n\
             \x20   (5 likes, 0 reposts, 0 quotes)"
        );
    }

    #[test]
    fn query_and_author_both_apply() {
        let out = render(&test_web(), Some("reply"), Some("bob"));
        assert!(out.starts_with(
            "=== SEARCH RESULTS ===\nquery: \"reply\" | author: bob | 2 matches in 4 posts"
        ));
        assert!(out.contains("Direct reply"));
        assert!(out.contains("Reply to quote"));

        let out = render(&test_web(), Some("reply"), Some("carol"));
        assert!(out.contains("| 0 matches in 4 posts"));
    }

    #[test]
    fn no_criteria_explains_itself() {
        let message = "No search criteria provided. Use --query and/or --author.";
        assert_eq!(render(&test_web(), None, None), message);
        assert_eq!(render(&test_web(), Some(""), Some("")), message);
    }

    #[test]
    fn no_matches_still_reports_the_filter() {
        let out = render(&test_web(), Some("nonexistent"), None);
        assert_eq!(
            out,
            "=== SEARCH RESULTS ===\nquery: \"nonexistent\" | 0 matches in 4 posts"
        );
    }

    #[test]
    fn matching_is_case_insensitive() {
        let out = render(&test_web(), Some("ORIGINAL"), None);
        assert!(out.contains("1 match"));
        assert!(out.contains("Original post"));

        let out = render(&test_web(), None, Some("CAROL"));
        assert!(out.contains("1 match"));
        assert!(out.contains("@carol.bsky.social"));
    }

    #[test]
    fn results_carry_thread_context() {
        let out = render(&test_web(), Some("Quote post"), None);
        assert!(out.contains("Thread: at://did:plc:c/app.bsky.feed.post/3 (2 posts)"));
    }

    #[test]
    fn posts_without_engagement_omit_the_stats_line() {
        let mut web = test_web();
        let post = web.get_post_mut(ROOT).unwrap();
        post.like_count = 0;
        post.repost_count = 0;
        post.quote_count = 0;
        let out = render(&web, Some("Original"), None);
        assert_eq!(
            out,
            "=== SEARCH RESULTS ===\n\
             query: \"Original\" | 1 match in 4 posts\n\
             \n\
             [1] Alice (@alice.bsky.social)  2026-01-15 10:00\n\
             \x20   Thread: at://did:plc:a/app.bsky.feed.post/1 (2 posts)\n\
             \x20   Original post"
        );
    }

    #[test]
    fn empty_web_reports_zero_matches() {
        let web = ContextWeb::new(ROOT, "2026-01-01T00:00:00Z");
        assert_eq!(
            render(&web, Some("anything"), None),
            "=== SEARCH RESULTS ===\nquery: \"anything\" | 0 matches in 0 posts"
        );
    }
}
