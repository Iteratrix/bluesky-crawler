use std::cmp::Reverse;
use std::collections::HashSet;

use indexmap::IndexMap;

use crate::model::{ContextWeb, Post, QuoteEdge};

use super::{author_name, short_time, thousands, truncate};

struct AuthorTotal {
    name: String,
    engagement: u64,
}

/// Surfaces the most notable posts and authors in the web.
pub(super) fn render(web: &ContextWeb, top: usize) -> String {
    let mut lines = vec!["=== HIGHLIGHTS ===".to_owned(), String::new()];
    most_quoted(web, top, &mut lines);
    most_replied(web, top, &mut lines);
    highest_engagement(web, top, &mut lines);
    main_characters(web, top, &mut lines);
    lines.join("\n").trim_end().to_owned()
}

fn most_quoted(web: &ContextWeb, top: usize, lines: &mut Vec<String>) {
    let counts = super::build_quotes_received(web);
    if counts.is_empty() {
        return;
    }

    let mut seen: HashSet<&str> = HashSet::new();
    let mut ranked: Vec<(&str, usize)> = Vec::new();
    for QuoteEdge { source, .. } in &web.quote_edges {
        let source = source.as_str();
        let Some(count) = counts.get(source) else {
            continue;
        };
        if seen.insert(source) {
            ranked.push((source, *count));
        }
    }
    ranked.sort_by_key(|(_, count)| Reverse(*count));

    lines.push("--- Most Quoted ---".to_owned());
    for (offset, (uri, count)) in ranked.iter().take(top).enumerate() {
        let Some(post) = web.get_post(uri) else {
            continue;
        };
        let rank = offset + 1;
        lines.push(format!(
            "  {rank}. [quoted {count} times] {}  {}",
            author_name(post),
            short_time(&post.created_at)
        ));
        lines.push(format!("     {}", truncate(&post.text, 80)));
        lines.push(format!(
            "     ({} likes, {} reposts)",
            thousands(post.like_count),
            thousands(post.repost_count)
        ));
        lines.push(String::new());
    }
}

fn most_replied(web: &ContextWeb, top: usize, lines: &mut Vec<String>) {
    let children = super::build_children(web);
    if children.is_empty() {
        return;
    }

    let mut seen: HashSet<&str> = HashSet::new();
    let mut ranked: Vec<(&str, usize)> = Vec::new();
    for post in web.iter_posts() {
        let Some(parent) = post.reply_parent.as_deref() else {
            continue;
        };
        let Some(replies) = children.get(parent) else {
            continue;
        };
        if seen.insert(parent) {
            ranked.push((parent, replies.len()));
        }
    }
    ranked.sort_by_key(|(_, count)| Reverse(*count));

    lines.push("--- Most Replied ---".to_owned());
    for (offset, (uri, count)) in ranked.iter().take(top).enumerate() {
        let Some(post) = web.get_post(uri) else {
            continue;
        };
        let rank = offset + 1;
        lines.push(format!(
            "  {rank}. [{count} replies in web] {}  {}",
            author_name(post),
            short_time(&post.created_at)
        ));
        lines.push(format!("     {}", truncate(&post.text, 80)));
        lines.push(String::new());
    }
}

fn highest_engagement(web: &ContextWeb, top: usize, lines: &mut Vec<String>) {
    let mut ranked: Vec<&Post> = web.iter_posts().collect();
    ranked.sort_by_key(|post| Reverse(post.engagement()));

    lines.push("--- Highest Engagement ---".to_owned());
    for (offset, post) in ranked.iter().take(top).enumerate() {
        let rank = offset + 1;
        lines.push(format!(
            "  {rank}. [{} likes, {} reposts, {} quotes] {}",
            thousands(post.like_count),
            thousands(post.repost_count),
            thousands(post.quote_count),
            author_name(post)
        ));
        lines.push(format!("     {}", truncate(&post.text, 80)));
        lines.push(String::new());
    }
}

fn main_characters(web: &ContextWeb, top: usize, lines: &mut Vec<String>) {
    let mut totals: IndexMap<&str, AuthorTotal> = IndexMap::new();
    for post in web.iter_posts() {
        let AuthorTotal { engagement, .. } = totals
            .entry(post.author.handle.as_str())
            .or_insert_with(|| AuthorTotal {
                name: author_name(post),
                engagement: 0,
            });
        *engagement += u64::from(post.engagement());
    }
    let mut ranked: Vec<AuthorTotal> = totals.into_values().collect();
    ranked.sort_by_key(|total| Reverse(total.engagement));

    lines.push("--- Main Characters (by total engagement) ---".to_owned());
    for (offset, AuthorTotal { name, engagement }) in ranked.iter().take(top).enumerate() {
        let rank = offset + 1;
        lines.push(format!(
            "  {rank}. {name} - {} total engagement",
            thousands(engagement)
        ));
    }
    lines.push(String::new());
}

#[cfg(test)]
mod tests {
    use crate::lens::fixtures::test_web;
    use crate::model::ContextWeb;

    use super::render;

    #[test]
    fn renders_every_section_for_the_fixture() {
        let out = render(&test_web(), 10);
        assert_eq!(
            out,
            "=== HIGHLIGHTS ===\n\
             \n\
             --- Most Quoted ---\n\
             \x20 1. [quoted 1 times] Alice (@alice.bsky.social)  2026-01-15 10:00\n\
             \x20    Original post\n\
             \x20    (10 likes, 3 reposts)\n\
             \n\
             --- Most Replied ---\n\
             \x20 1. [1 replies in web] Alice (@alice.bsky.social)  2026-01-15 10:00\n\
             \x20    Original post\n\
             \n\
             \x20 2. [1 replies in web] @carol.bsky.social  2026-01-15 10:08\n\
             \x20    Quote post\n\
             \n\
             --- Highest Engagement ---\n\
             \x20 1. [10 likes, 3 reposts, 1 quotes] Alice (@alice.bsky.social)\n\
             \x20    Original post\n\
             \n\
             \x20 2. [5 likes, 0 reposts, 0 quotes] @carol.bsky.social\n\
             \x20    Quote post\n\
             \n\
             \x20 3. [2 likes, 0 reposts, 0 quotes] Bob (@bob.bsky.social)\n\
             \x20    Direct reply\n\
             \n\
             \x20 4. [1 likes, 0 reposts, 0 quotes] Bob (@bob.bsky.social)\n\
             \x20    Reply to quote\n\
             \n\
             --- Main Characters (by total engagement) ---\n\
             \x20 1. Alice (@alice.bsky.social) - 14 total engagement\n\
             \x20 2. @carol.bsky.social - 5 total engagement\n\
             \x20 3. Bob (@bob.bsky.social) - 3 total engagement"
        );
    }

    #[test]
    fn section_headers_present() {
        let out = render(&test_web(), 10);
        assert!(out.contains("Most Quoted"));
        assert!(out.contains("quoted 1 times"));
        assert!(out.contains("Most Replied"));
        assert!(out.contains("1 replies in web"));
        assert!(out.contains("Highest Engagement"));
        assert!(out.contains("alice.bsky.social"));
        assert!(out.contains("Main Characters"));
    }

    #[test]
    fn top_one_keeps_headers_but_trims_entries() {
        let out = render(&test_web(), 1);
        assert!(out.contains("--- Most Quoted ---"));
        assert!(out.contains("--- Most Replied ---"));
        assert!(out.contains("--- Highest Engagement ---"));
        assert!(out.contains("--- Main Characters (by total engagement) ---"));
        assert!(!out.contains("  2. "));
        assert!(out.contains("  1. [1 replies in web] Alice (@alice.bsky.social)"));
    }

    #[test]
    fn top_zero_renders_headers_only() {
        let out = render(&test_web(), 0);
        assert_eq!(
            out,
            "=== HIGHLIGHTS ===\n\
             \n\
             --- Most Quoted ---\n\
             --- Most Replied ---\n\
             --- Highest Engagement ---\n\
             --- Main Characters (by total engagement) ---"
        );
    }

    #[test]
    fn top_larger_than_web_shows_everything() {
        let out = render(&test_web(), 99);
        assert_eq!(out.matches(" quotes] ").count(), 4);
        assert!(out.contains("  4. [1 likes, 0 reposts, 0 quotes] Bob (@bob.bsky.social)"));
    }

    #[test]
    fn empty_web_drops_quote_and_reply_sections() {
        let web = ContextWeb::new(
            "at://did:plc:a/app.bsky.feed.post/1",
            "2026-01-01T00:00:00Z",
        );
        assert_eq!(
            render(&web, 10),
            "=== HIGHLIGHTS ===\n\
             \n\
             --- Highest Engagement ---\n\
             --- Main Characters (by total engagement) ---"
        );
    }
}
