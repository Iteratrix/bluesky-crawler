"""Tests for local storage."""

import os

import pytest

from bsky_context.models import Author, ContextWeb, Post, Thread
from bsky_context.storage import list_webs, load_web, save_web, web_id


@pytest.fixture(autouse=True)
def tmp_data_dir(tmp_path):
    os.environ["XDG_DATA_HOME"] = str(tmp_path)
    yield
    del os.environ["XDG_DATA_HOME"]


def _make_web(root_uri: str = "at://did:plc:test/app.bsky.feed.post/abc123") -> ContextWeb:
    web = ContextWeb(root_uri=root_uri, crawled_at="2026-01-01T00:00:00Z")
    web.add_thread(Thread(
        root_uri=root_uri,
        posts={
            root_uri: Post(
                uri=root_uri, cid="cid1",
                author=Author(did="did:plc:test", handle="test.bsky.social"),
                text="Test post", created_at="2026-01-01T00:00:00Z",
            ),
        },
    ))
    return web


class TestWebId:
    def test_deterministic(self):
        uri = "at://did:plc:test/app.bsky.feed.post/abc123"
        assert web_id(uri) == web_id(uri)

    def test_contains_rkey(self):
        wid = web_id("at://did:plc:test/app.bsky.feed.post/abc123")
        assert wid.startswith("abc123-")

    def test_different_uris_differ(self):
        assert web_id("at://a/app.bsky.feed.post/x") != web_id("at://b/app.bsky.feed.post/x")


class TestSaveLoad:
    def test_roundtrip(self):
        web = _make_web()
        path = save_web(web)
        loaded = load_web(path.stem)
        assert loaded.node_count == 1
        assert loaded.thread_count == 1
        assert loaded.root_uri == web.root_uri

    def test_load_by_prefix(self):
        web = _make_web()
        save_web(web)
        loaded = load_web("abc123")
        assert loaded.node_count == 1

    def test_load_not_found(self):
        with pytest.raises(FileNotFoundError):
            load_web("nonexistent")

    def test_load_ambiguous(self):
        save_web(_make_web("at://did:plc:a/app.bsky.feed.post/abc1"))
        save_web(_make_web("at://did:plc:b/app.bsky.feed.post/abc2"))
        with pytest.raises(ValueError, match="Ambiguous"):
            load_web("abc")


class TestListWebs:
    def test_empty(self):
        assert list_webs() == []

    def test_lists_saved(self):
        save_web(_make_web())
        webs = list_webs()
        assert len(webs) == 1
        assert webs[0]["nodes"] == 1
        assert webs[0]["threads"] == 1

    def test_multiple(self):
        save_web(_make_web("at://did:plc:a/app.bsky.feed.post/first"))
        save_web(_make_web("at://did:plc:b/app.bsky.feed.post/second"))
        assert len(list_webs()) == 2
