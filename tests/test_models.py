"""Tests for data models."""

from bsky_context.models import Author, ContextWeb, Edge, EdgeType, Post


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


class TestEdge:
    def test_roundtrip(self):
        e = Edge(source="a", target="b", type=EdgeType.REPLY)
        assert Edge.from_dict(e.to_dict()) == e

    def test_quote_type(self):
        e = Edge(source="a", target="b", type=EdgeType.QUOTE)
        assert e.to_dict()["type"] == "quote"
        assert Edge.from_dict(e.to_dict()).type == EdgeType.QUOTE


class TestContextWeb:
    def test_empty_web(self):
        web = ContextWeb(root_uri="at://x/app.bsky.feed.post/1", crawled_at="2026-01-01T00:00:00Z")
        assert web.node_count == 0
        assert web.edge_count == 0

    def test_roundtrip(self):
        web = ContextWeb(root_uri="at://x/app.bsky.feed.post/1", crawled_at="2026-01-01T00:00:00Z")
        web.nodes["at://x/app.bsky.feed.post/1"] = _make_post("at://x/app.bsky.feed.post/1")
        web.nodes["at://x/app.bsky.feed.post/2"] = _make_post("at://x/app.bsky.feed.post/2")
        web.edges.append(Edge(source="at://x/app.bsky.feed.post/1", target="at://x/app.bsky.feed.post/2", type=EdgeType.REPLY))

        d = web.to_dict()
        web2 = ContextWeb.from_dict(d)
        assert web2.node_count == 2
        assert web2.edge_count == 1
        assert web2.root_uri == web.root_uri

    def test_deduplicate_edges(self):
        web = ContextWeb(root_uri="at://x/app.bsky.feed.post/1", crawled_at="2026-01-01T00:00:00Z")
        e = Edge(source="a", target="b", type=EdgeType.REPLY)
        web.edges = [e, e, e]
        web.deduplicate_edges()
        assert len(web.edges) == 1

    def test_deduplicate_preserves_different_types(self):
        web = ContextWeb(root_uri="at://x/app.bsky.feed.post/1", crawled_at="2026-01-01T00:00:00Z")
        web.edges = [
            Edge(source="a", target="b", type=EdgeType.REPLY),
            Edge(source="a", target="b", type=EdgeType.QUOTE),
        ]
        web.deduplicate_edges()
        assert len(web.edges) == 2

    def test_meta_in_serialized(self):
        web = ContextWeb(root_uri="at://x/app.bsky.feed.post/1", crawled_at="2026-01-01T00:00:00Z")
        web.nodes["at://x/app.bsky.feed.post/1"] = _make_post("at://x/app.bsky.feed.post/1")
        d = web.to_dict()
        assert d["meta"]["node_count"] == 1
        assert d["meta"]["edge_count"] == 0
        assert d["meta"]["root_uri"] == "at://x/app.bsky.feed.post/1"
