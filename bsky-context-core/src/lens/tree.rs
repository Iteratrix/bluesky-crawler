use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;

use super::{counted, split_lines};
use crate::model::{ContextWeb, Post, QuoteEdge};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EdgeKind {
    Reply,
    Quote,
}

impl EdgeKind {
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

#[derive(Debug, Clone, Copy)]
struct Child<'a> {
    uri: &'a str,
    kind: EdgeKind,
}

struct Graph<'a> {
    nodes: IndexMap<&'a str, &'a Post>,
    children: HashMap<&'a str, Vec<Child<'a>>>,
}

fn created_at<'a>(nodes: &IndexMap<&'a str, &'a Post>, uri: &str) -> &'a str {
    nodes.get(uri).map_or("", |post| post.created_at.as_str())
}

fn push_post(lines: &mut Vec<String>, post: &Post, depth: usize, kind: Option<EdgeKind>) {
    let indent = "  ".repeat(depth);
    let tag = match kind {
        Some(kind) => kind.tag(),
        None => "[root]",
    };
    let name = super::author_name(post);
    let time = super::short_time(&post.created_at);
    lines.push(format!("{indent}{tag} {name}  {time}"));

    for text_line in split_lines(&post.text) {
        lines.push(format!("{indent}  {text_line}"));
    }

    let Post {
        like_count,
        repost_count,
        quote_count,
        ..
    } = post;
    if *like_count > 0 || *repost_count > 0 || *quote_count > 0 {
        let mut stats: Vec<String> = Vec::new();
        if *like_count > 0 {
            stats.push(counted(*like_count, "like", "likes"));
        }
        if *repost_count > 0 {
            stats.push(counted(*repost_count, "repost", "reposts"));
        }
        if *quote_count > 0 {
            stats.push(counted(*quote_count, "quote", "quotes"));
        }
        lines.push(format!("{indent}  ({})", stats.join(", ")));
    }

    lines.push(String::new());
}

fn walk<'a>(
    graph: &Graph<'a>,
    start: &'a str,
    visited: &mut HashSet<&'a str>,
    lines: &mut Vec<String>,
) {
    let Graph { nodes, children } = graph;
    let mut stack: Vec<(&'a str, usize, Option<EdgeKind>)> = vec![(start, 0, None)];

    while let Some((uri, depth, kind)) = stack.pop() {
        if visited.contains(uri) {
            continue;
        }
        let Some(post) = nodes.get(uri) else {
            continue;
        };
        visited.insert(uri);
        push_post(lines, post, depth, kind);

        let Some(kids) = children.get(uri) else {
            continue;
        };
        let mut kids: Vec<Child<'a>> = kids.clone();
        kids.sort_by(|left, right| {
            left.kind
                .rank()
                .cmp(&right.kind.rank())
                .then_with(|| created_at(nodes, left.uri).cmp(created_at(nodes, right.uri)))
        });
        for Child { uri, kind } in kids.into_iter().rev() {
            stack.push((uri, depth + 1, Some(kind)));
        }
    }
}

/// Renders an indented threaded view: a depth-first walk from the tree root
/// with replies listed before quotes at every level.
pub(super) fn render(web: &ContextWeb) -> String {
    let mut children: HashMap<&str, Vec<Child<'_>>> = HashMap::new();
    for post in web.iter_posts() {
        let Some(parent) = &post.reply_parent else {
            continue;
        };
        children.entry(parent.as_str()).or_default().push(Child {
            uri: post.uri.as_str(),
            kind: EdgeKind::Reply,
        });
    }
    for QuoteEdge { source, target, .. } in &web.quote_edges {
        children.entry(source.as_str()).or_default().push(Child {
            uri: target.as_str(),
            kind: EdgeKind::Quote,
        });
    }

    let graph = Graph {
        nodes: web.nodes(),
        children,
    };
    let mut visited: HashSet<&str> = HashSet::new();
    let mut lines: Vec<String> = Vec::new();

    walk(&graph, super::find_tree_root(web), &mut visited, &mut lines);

    for &uri in graph.nodes.keys() {
        if visited.contains(uri) {
            continue;
        }
        lines.push("---".to_owned());
        walk(&graph, uri, &mut visited, &mut lines);
    }

    lines.join("\n").trim_end().to_owned()
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{QUOTE, REPLY, REPLY_TO_QUOTE, ROOT, author, test_web};
    use super::render;
    use crate::model::{ContextWeb, Post, QuoteEdge, Thread};

    const EXPECTED: &str = "\
[root] Alice (@alice.bsky.social)  2026-01-15 10:00
  Original post
  (10 likes, 3 reposts, 1 quote)

  [reply] Bob (@bob.bsky.social)  2026-01-15 10:05
    Direct reply
    (2 likes)

  [quote] @carol.bsky.social  2026-01-15 10:08
    Quote post
    (5 likes)

    [reply] Bob (@bob.bsky.social)  2026-01-15 10:12
      Reply to quote
      (1 like)";

    #[test]
    fn matches_reference_rendering() {
        assert_eq!(render(&test_web()), EXPECTED);
    }

    #[test]
    fn contains_all_posts() {
        let out = render(&test_web());
        for text in [
            "Original post",
            "Direct reply",
            "Quote post",
            "Reply to quote",
        ] {
            assert!(out.contains(text), "missing {text:?} in:\n{out}");
        }
    }

    #[test]
    fn root_tagged() {
        assert!(render(&test_web()).contains("[root]"));
    }

    #[test]
    fn reply_and_quote_tags() {
        let out = render(&test_web());
        assert!(out.contains("[reply]"));
        assert!(out.contains("[quote]"));
    }

    #[test]
    fn nesting_order() {
        let out = render(&test_web());
        let root_line = out
            .lines()
            .position(|line| line.contains("Original post"))
            .unwrap();
        let reply_line = out
            .lines()
            .position(|line| line.contains("Direct reply"))
            .unwrap();
        assert!(root_line < reply_line);
    }

    #[test]
    fn empty_web_renders_nothing() {
        let web = ContextWeb::new(ROOT, "2026-01-01T00:00:00Z");
        assert_eq!(render(&web), "");
    }

    #[test]
    fn replies_precede_quotes_even_when_later() {
        let mut web = test_web();
        web.get_post_mut(REPLY).unwrap().created_at = "2026-01-15T10:20:00Z".into();
        let out = render(&web);
        let reply = out.lines().position(|l| l.contains("[reply]")).unwrap();
        let quote = out.lines().position(|l| l.contains("[quote]")).unwrap();
        assert!(reply < quote, "quotes must sort after replies:\n{out}");
    }

    #[test]
    fn sibling_replies_sort_chronologically() {
        let mut web = test_web();
        let late = "at://did:plc:d/app.bsky.feed.post/5";
        let early = "at://did:plc:e/app.bsky.feed.post/6";
        let mut thread = web.remove_thread(ROOT).unwrap();
        thread.posts.insert(
            late.into(),
            Post {
                reply_parent: Some(ROOT.into()),
                ..Post::new(
                    late,
                    "c5",
                    author("did:plc:d", "dave.bsky.social", ""),
                    "Late sibling",
                    "2026-01-15T10:07:00Z",
                )
            },
        );
        thread.posts.insert(
            early.into(),
            Post {
                reply_parent: Some(ROOT.into()),
                ..Post::new(
                    early,
                    "c6",
                    author("did:plc:e", "erin.bsky.social", ""),
                    "Early sibling",
                    "2026-01-15T10:01:00Z",
                )
            },
        );
        web.add_thread(thread);

        let out = render(&web);
        let position = |needle: &str| out.lines().position(|l| l.contains(needle)).unwrap();
        assert!(position("Early sibling") < position("Direct reply"));
        assert!(position("Direct reply") < position("Late sibling"));
        assert!(position("Late sibling") < position("Quote post"));
    }

    #[test]
    fn disconnected_posts_follow_a_separator() {
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
        assert!(out.ends_with(
            "---\n[root] Dave (@dave.bsky.social)  2026-01-15 09:00\n  Orphan post\n  second line"
        ));
        assert_eq!(out.matches("---").count(), 1);
    }

    #[test]
    fn quote_edge_outside_the_web_is_ignored() {
        let mut web = test_web();
        web.quote_edges.push(QuoteEdge {
            source: REPLY.into(),
            target: "at://did:plc:x/app.bsky.feed.post/77".into(),
            source_thread: ROOT.into(),
            target_thread: "at://did:plc:x/app.bsky.feed.post/77".into(),
        });
        assert_eq!(render(&web), EXPECTED);
    }

    #[test]
    fn each_post_is_rendered_once() {
        let mut web = test_web();
        web.quote_edges.push(QuoteEdge {
            source: REPLY.into(),
            target: REPLY_TO_QUOTE.into(),
            source_thread: ROOT.into(),
            target_thread: QUOTE.into(),
        });
        assert_eq!(
            render(&web),
            "\
[root] Alice (@alice.bsky.social)  2026-01-15 10:00
  Original post
  (10 likes, 3 reposts, 1 quote)

  [reply] Bob (@bob.bsky.social)  2026-01-15 10:05
    Direct reply
    (2 likes)

    [quote] Bob (@bob.bsky.social)  2026-01-15 10:12
      Reply to quote
      (1 like)

  [quote] @carol.bsky.social  2026-01-15 10:08
    Quote post
    (5 likes)"
        );
    }
}
