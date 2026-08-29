use std::collections::HashMap;

use indexmap::IndexMap;

use crate::model::{ContextWeb, Post};

fn cross_reference(
    nodes: &IndexMap<&str, &Post>,
    uri_to_index: &HashMap<&str, usize>,
    uri: Option<&String>,
    verb: &str,
) -> Option<String> {
    let uri = uri?;
    let index = uri_to_index.get(uri.as_str())?;
    let handle = nodes
        .get(uri.as_str())
        .map_or_else(|| "?".to_owned(), |post| format!("@{}", post.author.handle));
    Some(format!("{verb} {handle} #{index}"))
}

/// Renders a chronological narrative: every post numbered, annotated with
/// the positions of the posts it replies to and quotes.
pub(super) fn render(web: &ContextWeb) -> String {
    let nodes = web.nodes();
    let mut posts: Vec<&Post> = nodes.values().copied().collect();
    posts.sort_by(|left, right| left.created_at.cmp(&right.created_at));
    let total = posts.len();
    let uri_to_index: HashMap<&str, usize> = posts
        .iter()
        .enumerate()
        .map(|(index, post)| (post.uri.as_str(), index + 1))
        .collect();

    let mut lines: Vec<String> = Vec::new();
    for (index, post) in posts.iter().enumerate() {
        let index = index + 1;
        let name = super::author_name(post);
        let time = super::short_time(&post.created_at);

        let context: Vec<String> = [
            cross_reference(
                &nodes,
                &uri_to_index,
                post.reply_parent.as_ref(),
                "replying to",
            ),
            cross_reference(&nodes, &uri_to_index, post.embed_uri.as_ref(), "quoting"),
        ]
        .into_iter()
        .flatten()
        .collect();
        let context = if context.is_empty() {
            String::new()
        } else {
            format!("  [{}]", context.join(", "))
        };

        lines.push(format!("[{index}/{total}] {name}  {time}{context}"));
        for text_line in post.text.lines() {
            lines.push(format!("  {text_line}"));
        }
        lines.push(String::new());
    }

    lines.join("\n").trim_end().to_owned()
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{QUOTE, REPLY, ROOT, author, test_web};
    use super::render;
    use crate::model::{ContextWeb, Post, Thread};

    const EXPECTED: &str = "\
[1/4] Alice (@alice.bsky.social)  2026-01-15 10:00
  Original post

[2/4] Bob (@bob.bsky.social)  2026-01-15 10:05  [replying to @alice.bsky.social #1]
  Direct reply

[3/4] @carol.bsky.social  2026-01-15 10:08  [quoting @alice.bsky.social #1]
  Quote post

[4/4] Bob (@bob.bsky.social)  2026-01-15 10:12  [replying to @carol.bsky.social #3]
  Reply to quote";

    #[test]
    fn matches_reference_rendering() {
        assert_eq!(render(&test_web()), EXPECTED);
    }

    #[test]
    fn sequential_numbering() {
        let out = render(&test_web());
        assert!(out.contains("[1/4]"));
        assert!(out.contains("[4/4]"));
    }

    #[test]
    fn chronological_order() {
        let out = render(&test_web());
        let headers: Vec<&str> = out.lines().filter(|line| line.starts_with('[')).collect();
        assert!(headers[0].contains("alice.bsky.social"));
        assert_eq!(headers.len(), 4);
    }

    #[test]
    fn cross_references() {
        let out = render(&test_web());
        assert!(out.contains("replying to"));
        assert!(out.contains("quoting"));
    }

    #[test]
    fn empty_web_renders_nothing() {
        let web = ContextWeb::new(ROOT, "2026-01-01T00:00:00Z");
        assert_eq!(render(&web), "");
    }

    #[test]
    fn numbering_follows_time_not_thread_order() {
        let mut web = test_web();
        web.get_post_mut(REPLY).unwrap().created_at = "2026-01-15T09:00:00Z".into();
        let out = render(&web);
        assert!(out.starts_with(
            "[1/4] Bob (@bob.bsky.social)  2026-01-15 09:00  \
             [replying to @alice.bsky.social #2]\n  Direct reply"
        ));
        assert!(
            out.contains("[2/4] Alice (@alice.bsky.social)  2026-01-15 10:00\n  Original post")
        );
    }

    #[test]
    fn missing_reply_parent_gets_no_annotation() {
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
        assert!(out.starts_with(
            "[1/5] Dave (@dave.bsky.social)  2026-01-15 09:00\n  Orphan post\n  second line"
        ));
        assert!(out.contains(
            "[3/5] Bob (@bob.bsky.social)  2026-01-15 10:05  [replying to @alice.bsky.social #2]"
        ));
    }

    #[test]
    fn quote_target_outside_the_web_gets_no_annotation() {
        let mut web = test_web();
        web.get_post_mut(QUOTE).unwrap().embed_uri =
            Some("at://did:plc:x/app.bsky.feed.post/77".into());
        let out = render(&web);
        assert!(out.contains("[3/4] @carol.bsky.social  2026-01-15 10:08\n  Quote post"));
        assert!(!out.contains("quoting"));
    }

    #[test]
    fn reply_and_quote_annotations_combine() {
        let mut web = test_web();
        let post = web.get_post_mut(QUOTE).unwrap();
        post.reply_parent = Some(REPLY.into());
        let out = render(&web);
        assert!(out.contains(
            "[3/4] @carol.bsky.social  2026-01-15 10:08  \
             [replying to @bob.bsky.social #2, quoting @alice.bsky.social #1]"
        ));
    }
}
