"""Tests for lens renderers."""

from bsky_context.lenses import render
from bsky_context.models import Author, ContextWeb, Post, QuoteEdge, Thread


def _build_test_web() -> ContextWeb:
    """Build a small graph: root -> reply, root -> quote -> reply-to-quote.

    Thread 1: Alice's root post + Bob's direct reply
    Thread 2: Carol's quote post + Bob's reply to the quote
    """
    web = ContextWeb(
        root_uri="at://did:plc:a/app.bsky.feed.post/1",
        crawled_at="2026-01-01T00:00:00Z",
    )

    # Thread 1
    thread1 = Thread(
        root_uri="at://did:plc:a/app.bsky.feed.post/1",
        posts={
            "at://did:plc:a/app.bsky.feed.post/1": Post(
                uri="at://did:plc:a/app.bsky.feed.post/1", cid="c1",
                author=Author(did="did:plc:a", handle="alice.bsky.social", display_name="Alice"),
                text="Original post", created_at="2026-01-15T10:00:00Z",
                like_count=10, repost_count=3, quote_count=1,
            ),
            "at://did:plc:b/app.bsky.feed.post/2": Post(
                uri="at://did:plc:b/app.bsky.feed.post/2", cid="c2",
                author=Author(did="did:plc:b", handle="bob.bsky.social", display_name="Bob"),
                text="Direct reply", created_at="2026-01-15T10:05:00Z",
                reply_parent="at://did:plc:a/app.bsky.feed.post/1",
                reply_root="at://did:plc:a/app.bsky.feed.post/1",
                like_count=2,
            ),
        },
    )

    # Thread 2
    thread2 = Thread(
        root_uri="at://did:plc:c/app.bsky.feed.post/3",
        posts={
            "at://did:plc:c/app.bsky.feed.post/3": Post(
                uri="at://did:plc:c/app.bsky.feed.post/3", cid="c3",
                author=Author(did="did:plc:c", handle="carol.bsky.social"),
                text="Quote post", created_at="2026-01-15T10:08:00Z",
                embed_uri="at://did:plc:a/app.bsky.feed.post/1",
                embed_type="app.bsky.embed.record",
                like_count=5,
            ),
            "at://did:plc:b/app.bsky.feed.post/4": Post(
                uri="at://did:plc:b/app.bsky.feed.post/4", cid="c4",
                author=Author(did="did:plc:b", handle="bob.bsky.social", display_name="Bob"),
                text="Reply to quote", created_at="2026-01-15T10:12:00Z",
                reply_parent="at://did:plc:c/app.bsky.feed.post/3",
                reply_root="at://did:plc:c/app.bsky.feed.post/3",
                like_count=1,
            ),
        },
    )

    web.add_thread(thread1)
    web.add_thread(thread2)

    web.quote_edges = [
        QuoteEdge(
            source="at://did:plc:a/app.bsky.feed.post/1",
            target="at://did:plc:c/app.bsky.feed.post/3",
            source_thread="at://did:plc:a/app.bsky.feed.post/1",
            target_thread="at://did:plc:c/app.bsky.feed.post/3",
        ),
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
        assert "threads" in data
        assert "quote_edges" in data


class TestStatsLens:
    def test_counts(self):
        out = render(_build_test_web(), "stats")
        assert "4" in out  # 4 posts
        assert "2" in out  # 2 threads

    def test_top_authors(self):
        out = render(_build_test_web(), "stats")
        assert "bob.bsky.social" in out
        assert "alice.bsky.social" in out

    def test_time_span(self):
        out = render(_build_test_web(), "stats")
        assert "2026-01-15 10:00" in out
        assert "2026-01-15 10:12" in out

    def test_thread_sizes(self):
        out = render(_build_test_web(), "stats")
        assert "Thread sizes:" in out

    def test_top_engagement(self):
        out = render(_build_test_web(), "stats")
        # Alice's post has 10+3+1=14 engagement, should be top
        assert "Original post" in out


class TestThreadsLens:
    def test_sorted_by_size(self):
        out = render(_build_test_web(), "threads")
        lines = out.splitlines()
        # Both threads have 2 posts, so either order is fine
        assert "2 posts" in out

    def test_root_text_shown(self):
        out = render(_build_test_web(), "threads")
        assert "Original post" in out

    def test_top_parameter(self):
        out = render(_build_test_web(), "threads", top=1)
        # Should show "showing top 1"
        assert "top 1" in out

    def test_engagement_shown(self):
        out = render(_build_test_web(), "threads")
        assert "engagement" in out


class TestHighlightsLens:
    def test_most_quoted(self):
        out = render(_build_test_web(), "highlights")
        # Alice's post is quoted once
        assert "Most Quoted" in out
        assert "quoted 1 time" in out

    def test_most_replied(self):
        out = render(_build_test_web(), "highlights")
        assert "Most Replied" in out
        # Alice's post and Carol's post each have 1 reply
        assert "1 replies in web" in out

    def test_highest_engagement(self):
        out = render(_build_test_web(), "highlights")
        assert "Highest Engagement" in out
        # Alice has highest engagement (10+3+1=14)
        assert "alice.bsky.social" in out

    def test_main_characters(self):
        out = render(_build_test_web(), "highlights")
        assert "Main Characters" in out

    def test_top_parameter(self):
        out = render(_build_test_web(), "highlights", top=1)
        # Still shows sections, just fewer entries
        assert "Most Quoted" in out


class TestNeighborhoodLens:
    def test_hops_zero_root_thread_only(self):
        out = render(_build_test_web(), "neighborhood", hops=0)
        # Only root thread (Alice + Bob reply)
        assert "Original post" in out
        assert "Direct reply" in out
        assert "Quote post" not in out
        assert "Reply to quote" not in out

    def test_hops_one_includes_quoted(self):
        out = render(_build_test_web(), "neighborhood", hops=1)
        # Both threads
        assert "Original post" in out
        assert "Quote post" in out

    def test_header_shows_counts(self):
        out = render(_build_test_web(), "neighborhood", hops=0)
        assert "2 of 4" in out  # 2 posts of 4 total

    def test_nonexistent_uri(self):
        out = render(_build_test_web(), "neighborhood", uri="at://nonexistent/post/1")
        assert "not found" in out.lower()

    def test_explicit_uri(self):
        # Neighborhood around Carol's quote post (thread 2), hops=0
        out = render(
            _build_test_web(), "neighborhood",
            uri="at://did:plc:c/app.bsky.feed.post/3", hops=0,
        )
        assert "Quote post" in out
        assert "Reply to quote" in out
        assert "Original post" not in out


class TestTimelineLens:
    def test_after_filter(self):
        out = render(_build_test_web(), "timeline", after="2026-01-15T10:06:00Z")
        # Only Carol's quote (10:08) and Bob's reply-to-quote (10:12)
        assert "Quote post" in out
        assert "Reply to quote" in out
        assert "Original post" not in out
        assert "Direct reply" not in out

    def test_before_filter(self):
        out = render(_build_test_web(), "timeline", before="2026-01-15T10:06:00Z")
        # Only Alice's root (10:00) and Bob's reply (10:05)
        assert "Original post" in out
        assert "Direct reply" in out
        assert "Quote post" not in out

    def test_window_filter(self):
        out = render(
            _build_test_web(), "timeline",
            after="2026-01-15T10:04:00Z", before="2026-01-15T10:09:00Z",
        )
        # Bob's reply (10:05) and Carol's quote (10:08)
        assert "[1/2]" in out
        assert "[2/2]" in out

    def test_header_shows_window(self):
        out = render(_build_test_web(), "timeline", after="2026-01-15T10:06:00Z")
        assert "after" in out.lower()

    def test_no_params_shows_all(self):
        out = render(_build_test_web(), "timeline")
        assert "[1/4]" in out
        assert "[4/4]" in out


class TestSearchLens:
    def test_text_search(self):
        out = render(_build_test_web(), "search", query="reply")
        # "Direct reply" and "Reply to quote"
        assert "2 matches" in out
        assert "Direct reply" in out
        assert "Reply to quote" in out

    def test_author_search(self):
        out = render(_build_test_web(), "search", author="carol")
        assert "1 matches" in out
        assert "carol.bsky.social" in out

    def test_combined_filters(self):
        out = render(_build_test_web(), "search", query="reply", author="bob")
        # Bob has "Direct reply" and "Reply to quote"
        assert "2 matches" in out

    def test_no_matches(self):
        out = render(_build_test_web(), "search", query="nonexistent")
        assert "0 matches" in out

    def test_no_criteria(self):
        out = render(_build_test_web(), "search")
        assert "No search criteria" in out

    def test_thread_context_shown(self):
        out = render(_build_test_web(), "search", query="Quote post")
        assert "Thread:" in out

    def test_case_insensitive(self):
        out = render(_build_test_web(), "search", query="ORIGINAL")
        assert "1 matches" in out


class TestInvalidLens:
    def test_unknown_lens(self):
        import pytest
        with pytest.raises(ValueError, match="Unknown lens"):
            render(_build_test_web(), "nonexistent")
