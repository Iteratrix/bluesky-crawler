use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;

use super::split_lines;
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
        let first_post = posts.iter().min_by_key(|post| &post.created_at);
        if first_post.is_some_and(|post| quote_targets.contains(post.uri.as_str())) {
            tags.push("joined via quote");
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

            let global = uri_to_index[post.uri.as_str()];
            let time = super::short_time(&post.created_at);
            lines.push(format!("  [{index}] (#{global}) {time}{context}"));
            for text_line in split_lines(&post.text) {
                lines.push(format!("    {text_line}"));
            }
            lines.push(String::new());
        }
    }

    lines.join("\n").trim_end().to_owned()
}

#[cfg(test)]
mod tests {
    #[test]
    fn joined_via_quote_marks_authors_whose_first_post_quotes() {
        use crate::model::{Post, QuoteEdge, Thread};
        let mut web = super::super::fixtures::test_web();
        let bob_quote = "at://did:plc:b/app.bsky.feed.post/7";
        let dave_quote = "at://did:plc:d/app.bsky.feed.post/8";
        let dave_reply = "at://did:plc:d/app.bsky.feed.post/9";
        let mut thread = Thread::new(bob_quote);
        thread.posts.insert(
            bob_quote.into(),
            Post {
                embed_uri: Some(super::super::fixtures::ROOT.into()),
                ..Post::new(
                    bob_quote,
                    "c7",
                    super::super::fixtures::author("did:plc:b", "bob.bsky.social", "Bob"),
                    "Bob quotes later",
                    "2026-01-15T11:00:00Z",
                )
            },
        );
        web.add_thread(thread);
        let mut thread = Thread::new(dave_quote);
        thread.posts.insert(
            dave_reply.into(),
            Post {
                reply_parent: Some(dave_quote.into()),
                reply_root: Some(dave_quote.into()),
                ..Post::new(
                    dave_reply,
                    "c9",
                    super::super::fixtures::author("did:plc:d", "dave.bsky.social", "Dave"),
                    "Dave replies to himself",
                    "2026-01-15T10:30:00Z",
                )
            },
        );
        thread.posts.insert(
            dave_quote.into(),
            Post {
                embed_uri: Some(super::super::fixtures::ROOT.into()),
                ..Post::new(
                    dave_quote,
                    "c8",
                    super::super::fixtures::author("did:plc:d", "dave.bsky.social", "Dave"),
                    "Dave quotes first",
                    "2026-01-15T10:20:00Z",
                )
            },
        );
        web.add_thread(thread);
        for target in [bob_quote, dave_quote] {
            web.quote_edges.push(QuoteEdge {
                source: super::super::fixtures::ROOT.into(),
                target: target.into(),
                source_thread: super::super::fixtures::ROOT.into(),
                target_thread: target.into(),
            });
        }
        let out = super::render(&web);
        assert!(out.contains("Bob (@bob.bsky.social) - 3 posts\n"), "{out}");
        assert!(
            out.contains("Dave (@dave.bsky.social) - 2 posts  [joined via quote]"),
            "{out}"
        );
        assert!(
            out.contains("@carol.bsky.social - 1 post  [joined via quote]"),
            "{out}"
        );
    }

    use super::super::fixtures::{QUOTE, REPLY, ROOT, author, test_web};
    use super::render;
    use crate::model::{ContextWeb, Post, Thread};

    const EXPECTED: &str = "\
=== PARTICIPANTS (3) ===
  Alice (@alice.bsky.social) - 1 post  [thread starter]
  Bob (@bob.bsky.social) - 2 posts
  @carol.bsky.social - 1 post  [joined via quote]

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
                "  @carol.bsky.social - 1 post  [joined via quote]",
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
        assert!(out.contains("@carol.bsky.social - 1 post  [joined via quote]"));
        assert!(!out.contains("Alice (@alice.bsky.social) - 1 post  [thread starter, via quote]"));
    }

    #[test]
    fn self_quoting_root_author_is_only_the_thread_starter() {
        let mut web = test_web();
        web.get_post_mut(QUOTE).unwrap().author = author("did:plc:a", "alice.bsky.social", "Alice");
        let out = render(&web);
        assert!(out.contains("Alice (@alice.bsky.social) - 2 posts  [thread starter]\n"));
        assert!(out.contains("PARTICIPANTS (2)"));
    }

    #[test]
    fn both_tags_combine_when_the_root_author_quoted_first() {
        let mut web = test_web();
        let quote = web.get_post_mut(QUOTE).unwrap();
        quote.author = author("did:plc:a", "alice.bsky.social", "Alice");
        quote.created_at = "2026-01-15T09:00:00Z".into();
        let out = render(&web);
        assert!(
            out.contains(
                "Alice (@alice.bsky.social) - 2 posts  [thread starter, joined via quote]"
            )
        );
    }

    #[test]
    fn missing_root_post_yields_no_thread_starter() {
        let mut web = test_web();
        web.root_uri = "at://did:plc:z/app.bsky.feed.post/99".into();
        let out = render(&web);
        assert!(!out.contains("thread starter"));
    }
}
