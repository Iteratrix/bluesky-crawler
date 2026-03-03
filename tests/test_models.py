"""Tests for data models."""

from bsky_context.models import Author, ContextWeb, Post, QuoteEdge, Thread


def _make_post(uri: str, text: str = "hello", **kwargs) -> Post:
    return Post(
        uri=uri,
        cid=f"cid-{uri[-3:]}",
        author=kwargs.pop("author", Author(did="did:plc:test", handle="test.bsky.social")),
        text=text,
        created_at="2026-01-01T00:00:00Z",
        **kwargs,
    )


class TestAuthor:
    def test_roundtrip(self):
        a = Author(did="did:plc:x", handle="x.bsky.social", display_name="X")
        assert Author.from_dict(a.to_dict()) == a

    def test_default_display_name(self):
        a = Author(did="did:plc:x", handle="x.bsky.social")
        assert a.display_name == ""


class TestPost:
    def test_roundtrip(self):
        p = _make_post("at://did:plc:a/app.bsky.feed.post/1", text="Hello world")
        assert Post.from_dict(p.to_dict()) == p

    def test_optional_fields_default_none(self):
        p = _make_post("at://did:plc:a/app.bsky.feed.post/1")
        assert p.reply_parent is None
        assert p.embed_uri is None

    def test_with_reply_refs(self):
        p = _make_post(
            "at://did:plc:a/app.bsky.feed.post/2",
            reply_parent="at://did:plc:a/app.bsky.feed.post/1",
            reply_root="at://did:plc:a/app.bsky.feed.post/1",
        )
        d = p.to_dict()
        p2 = Post.from_dict(d)
        assert p2.reply_parent == "at://did:plc:a/app.bsky.feed.post/1"


class TestThread:
    def test_empty_thread(self):
        t = Thread(root_uri="at://did:plc:a/app.bsky.feed.post/1")
        assert t.post_count == 0
        assert t.root_post is None

    def test_with_posts(self):
        root_uri = "at://did:plc:a/app.bsky.feed.post/1"
        t = Thread(root_uri=root_uri, posts={
            root_uri: _make_post(root_uri),
        })
        assert t.post_count == 1
        assert t.root_post is not None
        assert t.root_post.uri == root_uri

    def test_roundtrip(self):
        root_uri = "at://did:plc:a/app.bsky.feed.post/1"
        reply_uri = "at://did:plc:b/app.bsky.feed.post/2"
        t = Thread(root_uri=root_uri, posts={
            root_uri: _make_post(root_uri, text="root"),
            reply_uri: _make_post(reply_uri, text="reply",
                                  reply_parent=root_uri, reply_root=root_uri),
        })
        d = t.to_dict()
        t2 = Thread.from_dict(d)
        assert t2.root_uri == root_uri
        assert t2.post_count == 2
        assert t2.posts[reply_uri].reply_parent == root_uri


class TestQuoteEdge:
    def test_roundtrip(self):
        qe = QuoteEdge(
            source="at://a/app.bsky.feed.post/1",
            target="at://b/app.bsky.feed.post/2",
            source_thread="at://a/app.bsky.feed.post/1",
            target_thread="at://b/app.bsky.feed.post/2",
        )
        assert QuoteEdge.from_dict(qe.to_dict()) == qe


class TestContextWeb:
    def test_empty_web(self):
        web = ContextWeb(root_uri="at://x/app.bsky.feed.post/1", crawled_at="2026-01-01T00:00:00Z")
        assert web.node_count == 0
        assert web.edge_count == 0
        assert web.thread_count == 0

    def test_roundtrip(self):
        web = ContextWeb(root_uri="at://x/app.bsky.feed.post/1", crawled_at="2026-01-01T00:00:00Z")
        root_uri = "at://x/app.bsky.feed.post/1"
        reply_uri = "at://x/app.bsky.feed.post/2"
        web.threads[root_uri] = Thread(root_uri=root_uri, posts={
            root_uri: _make_post(root_uri),
            reply_uri: _make_post(reply_uri, reply_parent=root_uri, reply_root=root_uri),
        })

        d = web.to_dict()
        web2 = ContextWeb.from_dict(d)
        assert web2.node_count == 2
        assert web2.edge_count == 1  # one reply edge
        assert web2.thread_count == 1
        assert web2.root_uri == web.root_uri

    def test_node_count_across_threads(self):
        web = ContextWeb(root_uri="at://x/app.bsky.feed.post/1", crawled_at="2026-01-01T00:00:00Z")
        web.threads["at://a/app.bsky.feed.post/1"] = Thread(
            root_uri="at://a/app.bsky.feed.post/1",
            posts={"at://a/app.bsky.feed.post/1": _make_post("at://a/app.bsky.feed.post/1")},
        )
        web.threads["at://b/app.bsky.feed.post/2"] = Thread(
            root_uri="at://b/app.bsky.feed.post/2",
            posts={
                "at://b/app.bsky.feed.post/2": _make_post("at://b/app.bsky.feed.post/2"),
                "at://b/app.bsky.feed.post/3": _make_post("at://b/app.bsky.feed.post/3"),
            },
        )
        assert web.node_count == 3
        assert web.thread_count == 2

    def test_nodes_property(self):
        web = ContextWeb(root_uri="at://x/app.bsky.feed.post/1", crawled_at="2026-01-01T00:00:00Z")
        p1 = _make_post("at://a/app.bsky.feed.post/1")
        p2 = _make_post("at://b/app.bsky.feed.post/2")
        web.threads["at://a/app.bsky.feed.post/1"] = Thread(
            root_uri="at://a/app.bsky.feed.post/1", posts={p1.uri: p1},
        )
        web.threads["at://b/app.bsky.feed.post/2"] = Thread(
            root_uri="at://b/app.bsky.feed.post/2", posts={p2.uri: p2},
        )
        nodes = web.nodes
        assert len(nodes) == 2
        assert p1.uri in nodes
        assert p2.uri in nodes

    def test_thread_for_post(self):
        web = ContextWeb(root_uri="at://x/app.bsky.feed.post/1", crawled_at="2026-01-01T00:00:00Z")
        root_uri = "at://a/app.bsky.feed.post/1"
        web.threads[root_uri] = Thread(
            root_uri=root_uri, posts={root_uri: _make_post(root_uri)},
        )
        assert web.thread_for_post(root_uri) is not None
        assert web.thread_for_post(root_uri).root_uri == root_uri
        assert web.thread_for_post("at://nonexistent") is None

    def test_deduplicate_quote_edges(self):
        web = ContextWeb(root_uri="at://x/app.bsky.feed.post/1", crawled_at="2026-01-01T00:00:00Z")
        qe = QuoteEdge(source="a", target="b", source_thread="ta", target_thread="tb")
        web.quote_edges = [qe, qe, qe]
        web.deduplicate_quote_edges()
        assert len(web.quote_edges) == 1

    def test_deduplicate_preserves_distinct_edges(self):
        web = ContextWeb(root_uri="at://x/app.bsky.feed.post/1", crawled_at="2026-01-01T00:00:00Z")
        web.quote_edges = [
            QuoteEdge(source="a", target="b", source_thread="ta", target_thread="tb"),
            QuoteEdge(source="a", target="c", source_thread="ta", target_thread="tc"),
        ]
        web.deduplicate_quote_edges()
        assert len(web.quote_edges) == 2

    def test_edge_count_includes_replies_and_quotes(self):
        web = ContextWeb(root_uri="at://x/app.bsky.feed.post/1", crawled_at="2026-01-01T00:00:00Z")
        root = "at://x/app.bsky.feed.post/1"
        reply = "at://x/app.bsky.feed.post/2"
        web.threads[root] = Thread(root_uri=root, posts={
            root: _make_post(root),
            reply: _make_post(reply, reply_parent=root, reply_root=root),
        })
        web.quote_edges = [
            QuoteEdge(source=root, target="at://y/app.bsky.feed.post/3",
                      source_thread=root, target_thread="at://y/app.bsky.feed.post/3"),
        ]
        assert web.edge_count == 2  # 1 reply + 1 quote

    def test_meta_in_serialized(self):
        web = ContextWeb(root_uri="at://x/app.bsky.feed.post/1", crawled_at="2026-01-01T00:00:00Z")
        root = "at://x/app.bsky.feed.post/1"
        web.threads[root] = Thread(root_uri=root, posts={root: _make_post(root)})
        d = web.to_dict()
        assert d["meta"]["format_version"] == 2
        assert d["meta"]["node_count"] == 1
        assert d["meta"]["edge_count"] == 0
        assert d["meta"]["thread_count"] == 1
        assert d["meta"]["root_uri"] == "at://x/app.bsky.feed.post/1"
