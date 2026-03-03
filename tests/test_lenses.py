"""Tests for lens renderers."""

from bsky_context.lenses import render
from bsky_context.models import Author, ContextWeb, Edge, EdgeType, Post


def _build_test_web() -> ContextWeb:
    """Build a small graph: root -> reply, root -> quote -> reply-to-quote."""
    web = ContextWeb(
        root_uri="at://did:plc:a/app.bsky.feed.post/1",
        crawled_at="2026-01-01T00:00:00Z",
    )
    web.nodes["at://did:plc:a/app.bsky.feed.post/1"] = Post(
        uri="at://did:plc:a/app.bsky.feed.post/1", cid="c1",
        author=Author(did="did:plc:a", handle="alice.bsky.social", display_name="Alice"),
        text="Original post", created_at="2026-01-15T10:00:00Z",
        like_count=10,
    )
    web.nodes["at://did:plc:b/app.bsky.feed.post/2"] = Post(
        uri="at://did:plc:b/app.bsky.feed.post/2", cid="c2",
        author=Author(did="did:plc:b", handle="bob.bsky.social", display_name="Bob"),
        text="Direct reply", created_at="2026-01-15T10:05:00Z",
        reply_parent="at://did:plc:a/app.bsky.feed.post/1",
        reply_root="at://did:plc:a/app.bsky.feed.post/1",
    )
    web.nodes["at://did:plc:c/app.bsky.feed.post/3"] = Post(
        uri="at://did:plc:c/app.bsky.feed.post/3", cid="c3",
        author=Author(did="did:plc:c", handle="carol.bsky.social"),
        text="Quote post", created_at="2026-01-15T10:08:00Z",
        embed_uri="at://did:plc:a/app.bsky.feed.post/1",
        embed_type="app.bsky.embed.record",
    )
    web.nodes["at://did:plc:b/app.bsky.feed.post/4"] = Post(
        uri="at://did:plc:b/app.bsky.feed.post/4", cid="c4",
        author=Author(did="did:plc:b", handle="bob.bsky.social", display_name="Bob"),
        text="Reply to quote", created_at="2026-01-15T10:12:00Z",
        reply_parent="at://did:plc:c/app.bsky.feed.post/3",
        reply_root="at://did:plc:c/app.bsky.feed.post/3",
    )
    web.edges = [
        Edge(source="at://did:plc:a/app.bsky.feed.post/1", target="at://did:plc:b/app.bsky.feed.post/2", type=EdgeType.REPLY),
        Edge(source="at://did:plc:a/app.bsky.feed.post/1", target="at://did:plc:c/app.bsky.feed.post/3", type=EdgeType.QUOTE),
        Edge(source="at://did:plc:c/app.bsky.feed.post/3", target="at://did:plc:b/app.bsky.feed.post/4", type=EdgeType.REPLY),
    ]
    return web


class TestTreeLens:
    def test_contains_all_posts(self):
        out = render(_build_test_web(), "tree")
        assert "Original post" in out
        assert "Direct reply" in out
        assert "Quote post" in out
        assert "Reply to quote" in out

    def test_root_tagged(self):
        out = render(_build_test_web(), "tree")
        assert "[root]" in out

    def test_reply_and_quote_tags(self):
        out = render(_build_test_web(), "tree")
        assert "[reply]" in out
        assert "[quote]" in out

    def test_nesting_order(self):
        out = render(_build_test_web(), "tree")
        lines = out.splitlines()
        # Root should come before reply
        root_line = next(i for i, l in enumerate(lines) if "Original post" in l)
        reply_line = next(i for i, l in enumerate(lines) if "Direct reply" in l)
        assert root_line < reply_line


class TestLinearLens:
    def test_sequential_numbering(self):
        out = render(_build_test_web(), "linear")
        assert "[1/4]" in out
        assert "[4/4]" in out

    def test_chronological_order(self):
        out = render(_build_test_web(), "linear")
        lines = [l for l in out.splitlines() if l.startswith("[")]
        # First line should be the earliest post
        assert "alice.bsky.social" in lines[0]

    def test_cross_references(self):
        out = render(_build_test_web(), "linear")
        assert "replying to" in out
        assert "quoting" in out


class TestByAuthorLens:
    def test_participant_count(self):
        out = render(_build_test_web(), "by-author")
        assert "PARTICIPANTS (3)" in out

    def test_all_authors_present(self):
        out = render(_build_test_web(), "by-author")
        assert "alice.bsky.social" in out
        assert "bob.bsky.social" in out
        assert "carol.bsky.social" in out

    def test_thread_starter_tag(self):
        out = render(_build_test_web(), "by-author")
        assert "thread starter" in out

    def test_bob_has_two_posts(self):
        out = render(_build_test_web(), "by-author")
        assert "2 posts" in out


class TestRawLens:
    def test_valid_json(self):
        import json
        out = render(_build_test_web(), "raw")
        data = json.loads(out)
        assert "meta" in data
        assert "nodes" in data
        assert "edges" in data


class TestInvalidLens:
    def test_unknown_lens(self):
        import pytest
        with pytest.raises(ValueError, match="Unknown lens"):
            render(_build_test_web(), "nonexistent")
