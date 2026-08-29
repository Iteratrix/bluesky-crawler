use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;

use crate::model::{ContextWeb, Post, QuoteEdge};

fn mention(nodes: &IndexMap<&str, &Post>, uri: Option<&String>, verb: &str) -> Option<String> {
    let uri = uri?;
    let post = nodes.get(uri.as_str())?;
    Some(format!("{verb} @{}", post.author.handle))
}

/// Renders posts grouped by participant: a roster followed by each author's
/// posts in chronological order, cross-referenced to the linear numbering.
pub(super) fn render(web: &ContextWeb) -> String {
    let nodes = web.nodes();

    let mut by_author: IndexMap<&str, Vec<&Post>> = IndexMap::new();
    for post in nodes.values() {
        by_author
            .entry(post.author.did.as_str())
            .or_default()
            .push(post);
    }
    for posts in by_author.values_mut() {
        posts.sort_by(|left, right| left.created_at.cmp(&right.created_at));
    }

    let mut all_posts: Vec<&Post> = nodes.values().copied().collect();
    all_posts.sort_by(|left, right| left.created_at.cmp(&right.created_at));
    let uri_to_index: HashMap<&str, usize> = all_posts
        .iter()
        .enumerate()
        .map(|(index, post)| (post.uri.as_str(), index + 1))
        .collect();

    let mut author_order: Vec<(&str, Vec<&Post>)> = by_author.into_iter().collect();
    author_order.sort_by(|(_, left), (_, right)| left[0].created_at.cmp(&right[0].created_at));

    let root_did = nodes
        .get(super::find_tree_root(web))
        .map(|post| post.author.did.as_str());

    let quote_targets: HashSet<&str> = web
        .quote_edges
        .iter()
        .map(|QuoteEdge { target, .. }| target.as_str())
        .collect();

    let mut lines: Vec<String> = vec![format!("=== PARTICIPANTS ({}) ===", author_order.len())];
    for (did, posts) in &author_order {
        let name = super::author_name(posts[0]);
        let mut tags: Vec<&str> = Vec::new();
        if root_did == Some(*did) {
            tags.push("thread starter");
        }
        if posts
            .iter()
            .any(|post| quote_targets.contains(post.uri.as_str()))
        {
            tags.push("via quote");
        }
        let tags = if tags.is_empty() {
            String::new()
        } else {
            format!("  [{}]", tags.join(", "))
        };
        let count = posts.len();
        let plural = if count == 1 { "" } else { "s" };
        lines.push(format!("  {name} - {count} post{plural}{tags}"));
    }
    lines.push(String::new());

    for (_did, posts) in &author_order {
        lines.push(format!("=== {} ===", super::author_name(posts[0])));

        for (index, post) in posts.iter().enumerate() {
            let index = index + 1;
            let context: Vec<String> = [
                mention(&nodes, post.reply_parent.as_ref(), "replying to"),
                mention(&nodes, post.embed_uri.as_ref(), "quoting"),
            ]
            .into_iter()
            .flatten()
            .collect();
            let context = if context.is_empty() {
                String::new()
            } else {
                format!("  [{}]", context.join(", "))
            };

            let global = uri_to_index
                .get(post.uri.as_str())
                .map_or_else(|| "?".to_owned(), usize::to_string);
            let time = super::short_time(&post.created_at);
            lines.push(format!("  [{index}] (#{global}) {time}{context}"));
            for text_line in post.text.lines() {
                lines.push(format!("    {text_line}"));
            }
            lines.push(String::new());
        }
    }

    lines.join("\n").trim_end().to_owned()
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{QUOTE, REPLY, ROOT, author, test_web};
    use super::render;
    use crate::model::{ContextWeb, Post, Thread};

    const EXPECTED: &str = "\
=== PARTICIPANTS (3) ===
  Alice (@alice.bsky.social) - 1 post  [thread starter]
  Bob (@bob.bsky.social) - 2 posts
  @carol.bsky.social - 1 post  [via quote]

=== Alice (@alice.bsky.social) ===
  [1] (#1) 2026-01-15 10:00
    Original post

=== Bob (@bob.bsky.social) ===
  [1] (#2) 2026-01-15 10:05  [replying to @alice.bsky.social]
    Direct reply

  [2] (#4) 2026-01-15 10:12  [replying to @carol.bsky.social]
    Reply to quote

=== @carol.bsky.social ===
  [1] (#3) 2026-01-15 10:08  [quoting @alice.bsky.social]
    Quote post";

    #[test]
    fn matches_reference_rendering() {
        assert_eq!(render(&test_web()), EXPECTED);
    }

    #[test]
    fn participant_count() {
        assert!(render(&test_web()).contains("PARTICIPANTS (3)"));
    }

    #[test]
    fn all_authors_present() {
        let out = render(&test_web());
        for handle in ["alice.bsky.social", "bob.bsky.social", "carol.bsky.social"] {
            assert!(out.contains(handle), "missing {handle} in:\n{out}");
        }
    }

    #[test]
    fn thread_starter_tag() {
        assert!(render(&test_web()).contains("thread starter"));
    }

    #[test]
    fn bob_has_two_posts() {
        assert!(render(&test_web()).contains("2 posts"));
    }

    #[test]
    fn empty_web_lists_no_participants() {
        let web = ContextWeb::new(ROOT, "2026-01-01T00:00:00Z");
        assert_eq!(render(&web), "=== PARTICIPANTS (0) ===");
    }

    #[test]
    fn authors_ordered_by_first_post() {
        let mut web = test_web();
        web.get_post_mut(REPLY).unwrap().created_at = "2026-01-15T09:00:00Z".into();
        let out = render(&web);
        let roster: Vec<&str> = out.lines().skip(1).take(3).collect();
        assert_eq!(
            roster,
            vec![
                "  Bob (@bob.bsky.social) - 2 posts",
                "  Alice (@alice.bsky.social) - 1 post  [thread starter]",
                "  @carol.bsky.social - 1 post  [via quote]",
            ]
        );
    }

    #[test]
    fn a_new_author_joins_the_roster_at_their_first_post() {
        let mut web = test_web();
        let orphan = "at://did:plc:d/app.bsky.feed.post/9";
        let mut thread = Thread::new(orphan);
        thread.posts.insert(
            orphan.into(),
            Post {
                reply_parent: Some("at://did:plc:z/app.bsky.feed.post/99".into()),
                ..Post::new(
                    orphan,
                    "c9",
                    author("did:plc:d", "dave.bsky.social", "Dave"),
                    "Orphan post\nsecond line",
                    "2026-01-15T09:00:00Z",
                )
            },
        );
        web.add_thread(thread);

        let out = render(&web);
        assert!(
            out.starts_with(
                "=== PARTICIPANTS (4) ===\n  Dave (@dave.bsky.social) - 1 post\n  Alice"
            )
        );
        assert!(out.contains(
            "=== Dave (@dave.bsky.social) ===\n  [1] (#1) 2026-01-15 09:00\n    Orphan post\n    second line"
        ));
    }

    #[test]
    fn quoting_author_is_tagged_via_quote() {
        let out = render(&test_web());
        assert!(out.contains("@carol.bsky.social - 1 post  [via quote]"));
        assert!(!out.contains("Alice (@alice.bsky.social) - 1 post  [thread starter, via quote]"));
    }

    #[test]
    fn both_tags_combine_for_a_self_quoting_root_author() {
        let mut web = test_web();
        web.get_post_mut(QUOTE).unwrap().author = author("did:plc:a", "alice.bsky.social", "Alice");
        let out = render(&web);
        assert!(out.contains("Alice (@alice.bsky.social) - 2 posts  [thread starter, via quote]"));
        assert!(out.contains("PARTICIPANTS (2)"));
    }

    #[test]
    fn missing_root_post_yields_no_thread_starter() {
        let mut web = test_web();
        web.root_uri = "at://did:plc:z/app.bsky.feed.post/99".into();
        let out = render(&web);
        assert!(!out.contains("thread starter"));
    }
}
