use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;

use crate::model::{ContextWeb, Post, QuoteEdge};

use super::threads::comma;
use super::{author_name, short_time};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Edge {
    Reply,
    Quote,
}

impl Edge {
    fn tag(self) -> &'static str {
        match self {
            Self::Reply => "[reply]",
            Self::Quote => "[quote]",
        }
    }

    fn rank(self) -> u8 {
        match self {
            Self::Reply => 0,
            Self::Quote => 1,
        }
    }
}

/// Renders the posts within `hops` quote hops of a target post.
pub(super) fn render(web: &ContextWeb, uri: Option<&str>, hops: usize) -> String {
    let target_uri = match uri {
        Some(uri) if !uri.is_empty() => uri,
        Some(_) | None => web.root_uri.as_str(),
    };
    let Some(target_thread) = web.thread_root_for(target_uri) else {
        return format!("Post not found in web: {target_uri}");
    };

    let distances = super::thread_hop_distances(web, target_thread);
    let included: HashSet<&str> = distances
        .iter()
        .filter(|(_, distance)| **distance <= hops)
        .map(|(thread, _)| *thread)
        .collect();

    let mut nodes: IndexMap<&str, &Post> = IndexMap::new();
    for (root_uri, thread) in web.threads() {
        if !included.contains(root_uri.as_str()) {
            continue;
        }
        for (uri, post) in &thread.posts {
            nodes.insert(uri.as_str(), post);
        }
    }

    let mut children: HashMap<&str, Vec<(&str, Edge)>> = HashMap::new();
    for post in nodes.values() {
        let Some(parent) = post.reply_parent.as_deref() else {
            continue;
        };
        if !nodes.contains_key(parent) {
            continue;
        }
        children
            .entry(parent)
            .or_default()
            .push((post.uri.as_str(), Edge::Reply));
    }
    for QuoteEdge { source, target, .. } in &web.quote_edges {
        let (source, target) = (source.as_str(), target.as_str());
        if !nodes.contains_key(source) || !nodes.contains_key(target) {
            continue;
        }
        children
            .entry(source)
            .or_default()
            .push((target, Edge::Quote));
    }

    let mut lines = vec![
        format!("=== NEIGHBORHOOD ({hops} hops from target) ==="),
        format!(
            "Posts: {} of {} | Threads: {} of {}",
            comma(nodes.len()),
            comma(web.node_count()),
            comma(included.len()),
            comma(web.thread_count())
        ),
        String::new(),
    ];

    let tree_root = super::find_tree_root(web);
    let start = if nodes.contains_key(tree_root) {
        tree_root
    } else {
        target_uri
    };

    let mut visited: HashSet<&str> = HashSet::new();
    render_subtree(&nodes, &children, start, &mut visited, &mut lines);

    let uris: Vec<&str> = nodes.keys().copied().collect();
    for uri in uris {
        if visited.contains(uri) {
            continue;
        }
        lines.push("---".to_owned());
        render_subtree(&nodes, &children, uri, &mut visited, &mut lines);
    }

    lines.join("\n").trim_end().to_owned()
}

fn render_subtree<'a>(
    nodes: &IndexMap<&'a str, &'a Post>,
    children: &HashMap<&'a str, Vec<(&'a str, Edge)>>,
    start: &'a str,
    visited: &mut HashSet<&'a str>,
    lines: &mut Vec<String>,
) {
    let mut stack: Vec<(&'a str, usize, Option<Edge>)> = vec![(start, 0, None)];
    while let Some((uri, depth, edge)) = stack.pop() {
        let Some(post) = nodes.get(uri) else {
            continue;
        };
        if !visited.insert(uri) {
            continue;
        }

        let indent = "  ".repeat(depth);
        let tag = match edge {
            Some(edge) => edge.tag(),
            None => "[root]",
        };
        lines.push(format!(
            "{indent}{tag} {}  {}",
            author_name(post),
            short_time(&post.created_at)
        ));
        for text_line in post.text.lines() {
            lines.push(format!("{indent}  {text_line}"));
        }

        let mut stats: Vec<String> = Vec::new();
        if post.like_count > 0 {
            stats.push(format!("{} likes", post.like_count));
        }
        if post.repost_count > 0 {
            stats.push(format!("{} reposts", post.repost_count));
        }
        if post.quote_count > 0 {
            stats.push(format!("{} quotes", post.quote_count));
        }
        if !stats.is_empty() {
            lines.push(format!("{indent}  ({})", stats.join(", ")));
        }
        lines.push(String::new());

        let mut kids = children.get(uri).cloned().unwrap_or_default();
        kids.sort_by(|(a_uri, a_edge), (b_uri, b_edge)| {
            let a_time = nodes.get(a_uri).map_or("", |post| post.created_at.as_str());
            let b_time = nodes.get(b_uri).map_or("", |post| post.created_at.as_str());
            (a_edge.rank(), a_time).cmp(&(b_edge.rank(), b_time))
        });
        for (child_uri, child_edge) in kids.into_iter().rev() {
            stack.push((child_uri, depth + 1, Some(child_edge)));
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::lens::fixtures::{QUOTE, ROOT, author, test_web};
    use crate::model::{ContextWeb, Post};

    use super::render;

    #[test]
    fn hops_zero_keeps_the_root_thread_only() {
        let out = render(&test_web(), None, 0);
        assert_eq!(
            out,
            "=== NEIGHBORHOOD (0 hops from target) ===\n\
             Posts: 2 of 4 | Threads: 1 of 2\n\
             \n\
             [root] Alice (@alice.bsky.social)  2026-01-15 10:00\n\
             \x20 Original post\n\
             \x20 (10 likes, 3 reposts, 1 quotes)\n\
             \n\
             \x20 [reply] Bob (@bob.bsky.social)  2026-01-15 10:05\n\
             \x20   Direct reply\n\
             \x20   (2 likes)"
        );
        assert!(!out.contains("Quote post"));
        assert!(!out.contains("Reply to quote"));
    }

    #[test]
    fn hops_one_pulls_in_the_quoting_thread() {
        let out = render(&test_web(), None, 1);
        assert_eq!(
            out,
            "=== NEIGHBORHOOD (1 hops from target) ===\n\
             Posts: 4 of 4 | Threads: 2 of 2\n\
             \n\
             [root] Alice (@alice.bsky.social)  2026-01-15 10:00\n\
             \x20 Original post\n\
             \x20 (10 likes, 3 reposts, 1 quotes)\n\
             \n\
             \x20 [reply] Bob (@bob.bsky.social)  2026-01-15 10:05\n\
             \x20   Direct reply\n\
             \x20   (2 likes)\n\
             \n\
             \x20 [quote] @carol.bsky.social  2026-01-15 10:08\n\
             \x20   Quote post\n\
             \x20   (5 likes)\n\
             \n\
             \x20   [reply] Bob (@bob.bsky.social)  2026-01-15 10:12\n\
             \x20     Reply to quote\n\
             \x20     (1 likes)"
        );
    }

    #[test]
    fn hops_two_matches_hops_one_when_the_web_is_two_threads_deep() {
        let one = render(&test_web(), None, 1);
        let two = render(&test_web(), None, 2);
        assert_eq!(
            one.replace("(1 hops", "(N hops"),
            two.replace("(2 hops", "(N hops")
        );
    }

    #[test]
    fn header_shows_included_counts() {
        let out = render(&test_web(), None, 0);
        assert!(out.contains("2 of 4"));
        assert!(out.contains("1 of 2"));
    }

    #[test]
    fn unknown_uri_reports_not_found() {
        let out = render(&test_web(), Some("at://nonexistent/post/1"), 2);
        assert_eq!(out, "Post not found in web: at://nonexistent/post/1");
        assert!(out.to_lowercase().contains("not found"));
    }

    #[test]
    fn explicit_uri_centers_on_that_thread() {
        let out = render(&test_web(), Some(QUOTE), 0);
        assert_eq!(
            out,
            "=== NEIGHBORHOOD (0 hops from target) ===\n\
             Posts: 2 of 4 | Threads: 1 of 2\n\
             \n\
             [root] @carol.bsky.social  2026-01-15 10:08\n\
             \x20 Quote post\n\
             \x20 (5 likes)\n\
             \n\
             \x20 [reply] Bob (@bob.bsky.social)  2026-01-15 10:12\n\
             \x20   Reply to quote\n\
             \x20   (1 likes)"
        );
        assert!(!out.contains("Original post"));
    }

    #[test]
    fn empty_uri_falls_back_to_the_web_root() {
        assert_eq!(
            render(&test_web(), Some(""), 0),
            render(&test_web(), None, 0)
        );
        assert_eq!(
            render(&test_web(), Some(ROOT), 0),
            render(&test_web(), None, 0)
        );
    }

    #[test]
    fn posts_unreachable_from_the_root_are_separated_by_a_rule() {
        const ORPHAN: &str = "at://did:plc:z/app.bsky.feed.post/9";
        let mut web = test_web();
        web.add_post(
            ROOT,
            Post {
                reply_parent: Some("at://outside/x".into()),
                reply_root: Some("at://outside/x".into()),
                ..Post::new(
                    ORPHAN,
                    "c9",
                    author("did:plc:z", "zed.bsky.social", ""),
                    "Orphan post",
                    "2026-01-15T09:00:00Z",
                )
            },
        );
        let out = render(&web, None, 0);
        assert_eq!(
            out,
            "=== NEIGHBORHOOD (0 hops from target) ===\n\
             Posts: 3 of 5 | Threads: 1 of 2\n\
             \n\
             [root] Alice (@alice.bsky.social)  2026-01-15 10:00\n\
             \x20 Original post\n\
             \x20 (10 likes, 3 reposts, 1 quotes)\n\
             \n\
             \x20 [reply] Bob (@bob.bsky.social)  2026-01-15 10:05\n\
             \x20   Direct reply\n\
             \x20   (2 likes)\n\
             \n\
             ---\n\
             [root] @zed.bsky.social  2026-01-15 09:00\n\
             \x20 Orphan post"
        );
    }

    #[test]
    fn empty_web_reports_not_found() {
        let web = ContextWeb::new(ROOT, "2026-01-01T00:00:00Z");
        assert_eq!(
            render(&web, None, 2),
            format!("Post not found in web: {ROOT}")
        );
    }
}
