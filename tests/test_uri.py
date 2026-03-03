"""Tests for URI parsing."""

import pytest

from bsky_context.uri import PostRef


class TestPostRef:
    def test_parse_at_uri(self):
        ref = PostRef.from_str("at://did:plc:abc123/app.bsky.feed.post/xyz789")
        assert ref.repo == "did:plc:abc123"
        assert ref.rkey == "xyz789"

    def test_parse_bsky_url(self):
        ref = PostRef.from_str("https://bsky.app/profile/alice.bsky.social/post/abc")
        assert ref.repo == "alice.bsky.social"
        assert ref.rkey == "abc"

    def test_parse_bsky_url_http(self):
        ref = PostRef.from_str("http://bsky.app/profile/alice.bsky.social/post/abc")
        assert ref.repo == "alice.bsky.social"
        assert ref.rkey == "abc"

    def test_at_uri_property(self):
        ref = PostRef(repo="did:plc:abc", rkey="xyz")
        assert ref.at_uri == "at://did:plc:abc/app.bsky.feed.post/xyz"

    def test_roundtrip(self):
        original = "at://did:plc:abc123/app.bsky.feed.post/xyz789"
        ref = PostRef.from_str(original)
        assert ref.at_uri == original

    def test_strips_whitespace(self):
        ref = PostRef.from_str("  at://did:plc:abc/app.bsky.feed.post/xyz  ")
        assert ref.repo == "did:plc:abc"

    def test_invalid_input(self):
        with pytest.raises(ValueError, match="Cannot parse"):
            PostRef.from_str("not a valid uri")

    def test_invalid_empty(self):
        with pytest.raises(ValueError):
            PostRef.from_str("")

    def test_frozen(self):
        ref = PostRef(repo="did:plc:abc", rkey="xyz")
        with pytest.raises(AttributeError):
            ref.repo = "changed"  # type: ignore[misc]
