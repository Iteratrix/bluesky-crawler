"""Tests for the async crawler using mock Bluesky API responses."""

from __future__ import annotations

import types

import pytest

from atproto_client.exceptions import NetworkError, RequestException

from bsky_context.crawler import _retry, crawl
from bsky_context.models import Author, ContextWeb, Post, QuoteEdge, Thread

from conftest import (
    MockClient,
    at_uri,
    make_blocked,
    make_link_facet,
    make_not_found,
    make_post_view,
    make_thread_view,
)


# ===================================================================
# A. Graph Shape Tests
# ===================================================================


class TestGraphShapes:
    """Test that different thread topologies are correctly ingested."""

    async def test_singleton_post(self):
        """A1: Single post, no replies, no quotes (87.8% of real threads)."""
        pv = make_post_view("alice", "1", quote_count=0)
        client = MockClient()
        client.add_thread(at_uri("alice", "1"), make_thread_view(pv))

        web = await crawl(client, at_uri("alice", "1"))

        assert web.node_count == 1
        assert web.thread_count == 1
        assert web.edge_count == 0
        assert web.root_uri == at_uri("alice", "1")
        assert client.call_uris("get_quotes") == []

    async def test_linear_reply_chain(self):
        """A2: A→B→C linear chain in one thread."""
        a = make_post_view("alice", "1", quote_count=0)
        b = make_post_view(
            "bob", "2",
            reply_parent=at_uri("alice", "1"),
            reply_root=at_uri("alice", "1"),
            quote_count=0,
        )
        c = make_post_view(
            "carol", "3",
            reply_parent=at_uri("bob", "2"),
            reply_root=at_uri("alice", "1"),
            quote_count=0,
        )

        tree = make_thread_view(
            a,
            replies=[make_thread_view(b, replies=[make_thread_view(c)])],
        )

        client = MockClient()
        client.add_thread(at_uri("alice", "1"), tree)

        web = await crawl(client, at_uri("alice", "1"))

        assert web.node_count == 3
        assert web.thread_count == 1
        assert web.edge_count == 2  # two reply edges
        post_b = web.nodes[at_uri("bob", "2")]
        assert post_b.reply_parent == at_uri("alice", "1")
        post_c = web.nodes[at_uri("carol", "3")]
        assert post_c.reply_parent == at_uri("bob", "2")
        assert post_c.reply_root == at_uri("alice", "1")

    async def test_wide_reply_tree(self):
        """A3: Root with 5 direct replies (fan-out)."""
        root = make_post_view("alice", "1", quote_count=0)
        replies = []
        for i, name in enumerate(["bob", "carol", "dave", "eve", "frank"], start=2):
            rpv = make_post_view(
                name, str(i),
                reply_parent=at_uri("alice", "1"),
                reply_root=at_uri("alice", "1"),
                quote_count=0,
            )
            replies.append(make_thread_view(rpv))

        tree = make_thread_view(root, replies=replies)
        client = MockClient()
        client.add_thread(at_uri("alice", "1"), tree)

        web = await crawl(client, at_uri("alice", "1"))

        assert web.node_count == 6
        assert web.thread_count == 1
        assert web.edge_count == 5

    async def test_two_threads_linked_by_quote(self):
        """A4: Post B in T2 quotes Post A in T1. The fundamental Context Web shape."""
        a = make_post_view("alice", "1", quote_count=1)
        b = make_post_view("bob", "2", embed_uri=at_uri("alice", "1"), quote_count=0)

        client = MockClient()
        client.add_thread(at_uri("alice", "1"), make_thread_view(a))
        client.add_thread(at_uri("bob", "2"), make_thread_view(b))
        client.add_quotes(at_uri("alice", "1"), [b])

        web = await crawl(client, at_uri("alice", "1"))

        assert web.node_count == 2
        assert web.thread_count == 2
        assert len(web.quote_edges) >= 1
        # Check the quote edge direction: source=quoted, target=quoting
        qe = web.quote_edges[0]
        assert qe.source == at_uri("alice", "1")
        assert qe.target == at_uri("bob", "2")

    async def test_quote_chain(self):
        """A5: A is quoted by B, B is quoted by C. Three separate threads."""
        a = make_post_view("alice", "1", quote_count=1)
        b = make_post_view("bob", "2", embed_uri=at_uri("alice", "1"), quote_count=1)
        c = make_post_view("carol", "3", embed_uri=at_uri("bob", "2"), quote_count=0)

        client = MockClient()
        client.add_thread(at_uri("alice", "1"), make_thread_view(a))
        client.add_thread(at_uri("bob", "2"), make_thread_view(b))
        client.add_thread(at_uri("carol", "3"), make_thread_view(c))
        client.add_quotes(at_uri("alice", "1"), [b])
        client.add_quotes(at_uri("bob", "2"), [c])

        web = await crawl(client, at_uri("alice", "1"))

        assert web.node_count == 3
        assert web.thread_count == 3
        assert len(web.quote_edges) == 2
        sources = {qe.source for qe in web.quote_edges}
        targets = {qe.target for qe in web.quote_edges}
        assert at_uri("alice", "1") in sources
        assert at_uri("bob", "2") in sources
        assert at_uri("bob", "2") in targets
        assert at_uri("carol", "3") in targets

    async def test_intra_thread_quote(self):
        """A6: B replies to A AND quotes A — both in the same thread."""
        a = make_post_view("alice", "1", quote_count=1)
        b = make_post_view(
            "bob", "2",
            reply_parent=at_uri("alice", "1"),
            reply_root=at_uri("alice", "1"),
            embed_uri=at_uri("alice", "1"),
            quote_count=0,
        )

        tree = make_thread_view(a, replies=[make_thread_view(b)])
        client = MockClient()
        client.add_thread(at_uri("alice", "1"), tree)
        client.add_quotes(at_uri("alice", "1"), [b])

        web = await crawl(client, at_uri("alice", "1"))

        assert web.thread_count == 1
        assert web.node_count == 2
        # Intra-thread quote: source_thread == target_thread
        assert len(web.quote_edges) >= 1
        qe = web.quote_edges[0]
        assert qe.source_thread == qe.target_thread

    async def test_mid_thread_fetch(self):
        """A7: Start from B (mid-thread). API returns parent A + replies C,D."""
        a = make_post_view("alice", "1", quote_count=0)
        b = make_post_view(
            "bob", "2",
            reply_parent=at_uri("alice", "1"),
            reply_root=at_uri("alice", "1"),
            quote_count=0,
        )
        c = make_post_view(
            "carol", "3",
            reply_parent=at_uri("bob", "2"),
            reply_root=at_uri("alice", "1"),
            quote_count=0,
        )
        d = make_post_view(
            "dave", "4",
            reply_parent=at_uri("bob", "2"),
            reply_root=at_uri("alice", "1"),
            quote_count=0,
        )

        # API response from B's perspective: parent=A, replies=[C, D]
        tree = make_thread_view(
            b,
            parent=make_thread_view(a),
            replies=[make_thread_view(c), make_thread_view(d)],
        )
        client = MockClient()
        client.add_thread(at_uri("bob", "2"), tree)

        web = await crawl(client, at_uri("bob", "2"))

        assert web.node_count == 4
        assert web.thread_count == 1
        # Thread root should be A (topmost ancestor)
        thread = list(web.threads.values())[0]
        assert thread.root_uri == at_uri("alice", "1")

    async def test_not_found_in_parent_chain(self):
        """A8: Parent is a NotFoundPost — child is collected, no crash."""
        c = make_post_view(
            "carol", "3",
            reply_parent=at_uri("missing", "2"),
            reply_root=at_uri("missing", "2"),
            quote_count=0,
        )
        tree = make_thread_view(
            c,
            parent=make_not_found(at_uri("missing", "2")),
        )
        client = MockClient()
        client.add_thread(at_uri("carol", "3"), tree)

        web = await crawl(client, at_uri("carol", "3"))

        # Only C collected; the NotFoundPost parent is skipped
        assert web.node_count == 1
        assert at_uri("carol", "3") in web.nodes

    async def test_blocked_post_in_replies(self):
        """A9: One reply is blocked — it's skipped, others collected."""
        root = make_post_view("alice", "1", quote_count=0)
        good_reply = make_post_view(
            "bob", "2",
            reply_parent=at_uri("alice", "1"),
            reply_root=at_uri("alice", "1"),
            quote_count=0,
        )
        tree = make_thread_view(
            root,
            replies=[
                make_blocked(at_uri("blocked", "99")),
                make_thread_view(good_reply),
            ],
        )
        client = MockClient()
        client.add_thread(at_uri("alice", "1"), tree)

        web = await crawl(client, at_uri("alice", "1"))

        assert web.node_count == 2  # root + good reply, blocked skipped


# ===================================================================
# B. Crawler Mechanics Tests
# ===================================================================


class TestCrawlerMechanics:
    """Test BFS logic: dedup, limits, smart re-fetch, merging."""

    async def test_thread_level_dedup(self):
        """B1: Two posts in same thread are both queued — thread fetched only once."""
        # Thread T1: A (root) + B (reply)
        a = make_post_view("alice", "1", quote_count=1)
        b = make_post_view(
            "bob", "2",
            reply_parent=at_uri("alice", "1"),
            reply_root=at_uri("alice", "1"),
            quote_count=1,
        )
        t1 = make_thread_view(a, replies=[make_thread_view(b)])
        # C quotes A, D quotes B — both discover T1 posts as embed targets
        c = make_post_view("carol", "3", embed_uri=at_uri("alice", "1"), quote_count=0)
        d = make_post_view("dave", "4", embed_uri=at_uri("bob", "2"), quote_count=0)

        client = MockClient()
        client.add_thread(at_uri("alice", "1"), t1)
        client.add_thread(at_uri("bob", "2"), t1)  # same thread from either URI
        client.add_thread(at_uri("carol", "3"), make_thread_view(c))
        client.add_thread(at_uri("dave", "4"), make_thread_view(d))
        client.add_quotes(at_uri("alice", "1"), [c])
        client.add_quotes(at_uri("bob", "2"), [d])

        web = await crawl(client, at_uri("alice", "1"))

        assert web.node_count == 4
        # T1 should only be fetched once (for alice/1), not again for bob/2
        thread_uris = set(client.call_uris("get_post_thread"))
        assert at_uri("alice", "1") in thread_uris
        # bob/2 should NOT trigger a separate getPostThread (it's in T1)
        assert at_uri("bob", "2") not in thread_uris

    async def test_max_depth(self):
        """B2: max_depth limits BFS hop distance."""
        # Quote chain: A → B → C → D
        a = make_post_view("alice", "1", quote_count=1)
        b = make_post_view("bob", "2", embed_uri=at_uri("alice", "1"), quote_count=1)
        c = make_post_view("carol", "3", embed_uri=at_uri("bob", "2"), quote_count=1)
        d = make_post_view("dave", "4", embed_uri=at_uri("carol", "3"), quote_count=0)

        client = MockClient()
        client.add_thread(at_uri("alice", "1"), make_thread_view(a))
        client.add_thread(at_uri("bob", "2"), make_thread_view(b))
        client.add_thread(at_uri("carol", "3"), make_thread_view(c))
        client.add_thread(at_uri("dave", "4"), make_thread_view(d))
        client.add_quotes(at_uri("alice", "1"), [b])
        client.add_quotes(at_uri("bob", "2"), [c])
        client.add_quotes(at_uri("carol", "3"), [d])

        web = await crawl(client, at_uri("alice", "1"), max_depth=1)

        # depth 0: A, depth 1: B (queued and explored).
        # C is discovered by getQuotes(B) and added to web, but NOT queued further.
        # D is never seen because C is never explored.
        assert web.node_count == 3
        assert at_uri("alice", "1") in web.nodes
        assert at_uri("bob", "2") in web.nodes
        assert at_uri("carol", "3") in web.nodes
        assert at_uri("dave", "4") not in web.nodes

    async def test_max_depth_zero(self):
        """B2b: max_depth=0 only collects the start post."""
        a = make_post_view("alice", "1", quote_count=1)
        b = make_post_view("bob", "2", embed_uri=at_uri("alice", "1"), quote_count=0)

        client = MockClient()
        client.add_thread(at_uri("alice", "1"), make_thread_view(a))
        client.add_thread(at_uri("bob", "2"), make_thread_view(b))
        client.add_quotes(at_uri("alice", "1"), [b])

        web = await crawl(client, at_uri("alice", "1"), max_depth=0)

        # A is explored at depth 0. getQuotes(A) discovers B and adds it to
        # the web, but B is NOT queued for further exploration (depth 1 > 0).
        assert web.node_count == 2

    async def test_max_nodes(self):
        """B3: max_nodes caps total posts collected."""
        # Quote chain: each post is its own thread
        posts = []
        for i in range(10):
            name = f"user{i}"
            embed = at_uri(f"user{i-1}", str(i - 1)) if i > 0 else None
            qc = 1 if i < 9 else 0
            pv = make_post_view(name, str(i), embed_uri=embed, quote_count=qc)
            posts.append(pv)

        client = MockClient()
        for i, pv in enumerate(posts):
            client.add_thread(at_uri(f"user{i}", str(i)), make_thread_view(pv))
            if i < 9:
                client.add_quotes(at_uri(f"user{i}", str(i)), [posts[i + 1]])

        web = await crawl(client, at_uri("user0", "0"), max_nodes=3)

        assert web.node_count <= 4  # may overshoot by 1 thread
        assert web.node_count >= 3

    async def test_smart_refetch_skips_unchanged_quotes(self):
        """B4: Existing web with edges matching quote_count → getQuotes skipped."""
        existing = ContextWeb(
            root_uri=at_uri("alice", "1"),
            crawled_at="2026-01-01T00:00:00Z",
        )
        existing.add_thread(Thread(
            root_uri=at_uri("alice", "1"),
            posts={
                at_uri("alice", "1"): Post(
                    uri=at_uri("alice", "1"),
                    cid="cid-alice-1",
                    author=Author(did="did:plc:alice", handle="alice.bsky.social"),
                    text="hi",
                    created_at="2026-01-01T00:00:00Z",
                    quote_count=1,
                )
            },
        ))
        # Existing edge proves we already checked quotes for alice/1
        existing.quote_edges.append(QuoteEdge(
            source=at_uri("alice", "1"),
            target=at_uri("prev", "99"),
            source_thread=at_uri("alice", "1"),
            target_thread=at_uri("prev", "99"),
        ))

        # API returns same quote_count=1
        a = make_post_view("alice", "1", quote_count=1)
        client = MockClient()
        client.add_thread(at_uri("alice", "1"), make_thread_view(a))
        # Register quotes that should NOT be fetched
        client.add_quotes(at_uri("alice", "1"), [make_post_view("bob", "2")])

        web = await crawl(client, at_uri("alice", "1"), existing=existing)

        assert client.call_uris("get_quotes") == []

    async def test_smart_refetch_fetches_new_quotes(self):
        """B5: Existing web with fewer edges than quote_count → getQuotes IS called."""
        existing = ContextWeb(
            root_uri=at_uri("alice", "1"),
            crawled_at="2026-01-01T00:00:00Z",
        )
        existing.add_thread(Thread(
            root_uri=at_uri("alice", "1"),
            posts={
                at_uri("alice", "1"): Post(
                    uri=at_uri("alice", "1"),
                    cid="cid-alice-1",
                    author=Author(did="did:plc:alice", handle="alice.bsky.social"),
                    text="hi",
                    created_at="2026-01-01T00:00:00Z",
                    quote_count=2,  # old count
                )
            },
        ))
        # 2 existing edges from previous crawl
        for i in range(2):
            existing.quote_edges.append(QuoteEdge(
                source=at_uri("alice", "1"),
                target=at_uri("prev", str(i)),
                source_thread=at_uri("alice", "1"),
                target_thread=at_uri("prev", str(i)),
            ))

        # API returns higher quote_count=5
        a = make_post_view("alice", "1", quote_count=5)
        b = make_post_view("bob", "2", embed_uri=at_uri("alice", "1"), quote_count=0)
        client = MockClient()
        client.add_thread(at_uri("alice", "1"), make_thread_view(a))
        client.add_thread(at_uri("bob", "2"), make_thread_view(b))
        client.add_quotes(at_uri("alice", "1"), [b])

        web = await crawl(client, at_uri("alice", "1"), existing=existing)

        assert at_uri("alice", "1") in client.call_uris("get_quotes")
        assert web.node_count == 2

    async def test_smart_refetch_no_edges_means_unexplored(self):
        """B5b: Existing post with quote_count>0 but no edges → getQuotes called.

        This is the timeout-then-resume case: a post was discovered but its
        quotes were never followed because the crawl timed out.
        """
        existing = ContextWeb(
            root_uri=at_uri("alice", "1"),
            crawled_at="2026-01-01T00:00:00Z",
        )
        existing.add_thread(Thread(
            root_uri=at_uri("alice", "1"),
            posts={
                at_uri("alice", "1"): Post(
                    uri=at_uri("alice", "1"),
                    cid="cid-alice-1",
                    author=Author(did="did:plc:alice", handle="alice.bsky.social"),
                    text="hi",
                    created_at="2026-01-01T00:00:00Z",
                    quote_count=3,  # discovered but never explored
                )
            },
        ))
        # No quote_edges — simulates a timeout before getQuotes was called

        # API returns same quote_count=3
        a = make_post_view("alice", "1", quote_count=3)
        b = make_post_view("bob", "2", embed_uri=at_uri("alice", "1"), quote_count=0)
        client = MockClient()
        client.add_thread(at_uri("alice", "1"), make_thread_view(a))
        client.add_thread(at_uri("bob", "2"), make_thread_view(b))
        client.add_quotes(at_uri("alice", "1"), [b])

        web = await crawl(client, at_uri("alice", "1"), existing=existing)

        # getQuotes MUST be called — no edges means we never checked
        assert at_uri("alice", "1") in client.call_uris("get_quotes")
        assert web.node_count == 2

    async def test_smart_refetch_mixed_explored_and_unexplored(self):
        """B5c: Two posts — one with edges (explored), one without (unexplored).

        Only the unexplored post should trigger getQuotes.
        """
        existing = ContextWeb(
            root_uri=at_uri("alice", "1"),
            crawled_at="2026-01-01T00:00:00Z",
        )
        existing.add_thread(Thread(
            root_uri=at_uri("alice", "1"),
            posts={
                at_uri("alice", "1"): Post(
                    uri=at_uri("alice", "1"),
                    cid="cid-alice-1",
                    author=Author(did="did:plc:alice", handle="alice.bsky.social"),
                    text="root",
                    created_at="2026-01-01T00:00:00Z",
                    quote_count=1,  # explored — has edge below
                ),
                at_uri("alice", "5"): Post(
                    uri=at_uri("alice", "5"),
                    cid="cid-alice-5",
                    author=Author(did="did:plc:alice", handle="alice.bsky.social"),
                    text="reply",
                    created_at="2026-01-01T00:01:00Z",
                    reply_parent=at_uri("alice", "1"),
                    reply_root=at_uri("alice", "1"),
                    quote_count=2,  # NOT explored — timed out
                ),
            },
        ))
        # Edge for alice/1 — proves it was explored
        existing.quote_edges.append(QuoteEdge(
            source=at_uri("alice", "1"),
            target=at_uri("prev", "99"),
            source_thread=at_uri("alice", "1"),
            target_thread=at_uri("prev", "99"),
        ))
        # No edges for alice/5 — never explored

        # API returns same quote counts
        a = make_post_view("alice", "1", quote_count=1)
        a5 = make_post_view(
            "alice", "5",
            reply_parent=at_uri("alice", "1"),
            reply_root=at_uri("alice", "1"),
            quote_count=2,
        )
        b = make_post_view("bob", "2", embed_uri=at_uri("alice", "5"), quote_count=0)
        c = make_post_view("carol", "3", embed_uri=at_uri("alice", "5"), quote_count=0)

        client = MockClient()
        client.add_thread(
            at_uri("alice", "1"),
            make_thread_view(a, replies=[make_thread_view(a5)]),
        )
        client.add_thread(at_uri("bob", "2"), make_thread_view(b))
        client.add_thread(at_uri("carol", "3"), make_thread_view(c))
        # Quotes that should NOT be fetched (alice/1 already explored)
        client.add_quotes(at_uri("alice", "1"), [make_post_view("skip", "99")])
        # Quotes that SHOULD be fetched (alice/5 never explored)
        client.add_quotes(at_uri("alice", "5"), [b, c])

        web = await crawl(client, at_uri("alice", "1"), existing=existing)

        quote_uris = client.call_uris("get_quotes")
        assert at_uri("alice", "1") not in quote_uris  # skipped — has edge
        assert at_uri("alice", "5") in quote_uris  # checked — no edges
        assert web.node_count >= 4  # alice/1, alice/5, bob/2, carol/3

    async def test_smart_refetch_edges_exist_but_count_grew(self):
        """B5d: Post has edges from prior crawl but quote_count increased → re-check."""
        existing = ContextWeb(
            root_uri=at_uri("alice", "1"),
            crawled_at="2026-01-01T00:00:00Z",
        )
        existing.add_thread(Thread(
            root_uri=at_uri("alice", "1"),
            posts={
                at_uri("alice", "1"): Post(
                    uri=at_uri("alice", "1"),
                    cid="cid-alice-1",
                    author=Author(did="did:plc:alice", handle="alice.bsky.social"),
                    text="hi",
                    created_at="2026-01-01T00:00:00Z",
                    quote_count=1,  # was 1 when we last checked
                )
            },
        ))
        # 1 edge from previous crawl
        existing.quote_edges.append(QuoteEdge(
            source=at_uri("alice", "1"),
            target=at_uri("prev", "99"),
            source_thread=at_uri("alice", "1"),
            target_thread=at_uri("prev", "99"),
        ))

        # API now returns quote_count=3 (grew from 1 to 3)
        a = make_post_view("alice", "1", quote_count=3)
        b = make_post_view("bob", "2", embed_uri=at_uri("alice", "1"), quote_count=0)
        client = MockClient()
        client.add_thread(at_uri("alice", "1"), make_thread_view(a))
        client.add_thread(at_uri("bob", "2"), make_thread_view(b))
        client.add_quotes(at_uri("alice", "1"), [b])

        web = await crawl(client, at_uri("alice", "1"), existing=existing)

        assert at_uri("alice", "1") in client.call_uris("get_quotes")
        assert web.node_count == 2

    async def test_quote_count_zero_skips_get_quotes(self):
        """B6: Posts with quote_count=0 never trigger getQuotes."""
        a = make_post_view("alice", "1", quote_count=0)
        b = make_post_view(
            "bob", "2",
            reply_parent=at_uri("alice", "1"),
            reply_root=at_uri("alice", "1"),
            quote_count=0,
        )
        tree = make_thread_view(a, replies=[make_thread_view(b)])
        client = MockClient()
        client.add_thread(at_uri("alice", "1"), tree)

        web = await crawl(client, at_uri("alice", "1"))

        assert web.node_count == 2
        assert client.calls("get_quotes") == []

    async def test_placeholder_thread_merging(self):
        """B7: getQuotes creates placeholder thread, _fetch_thread merges it."""
        # A is in its own thread, has 1 quote
        a = make_post_view("alice", "1", quote_count=1)
        # C replies to B and quotes A. getQuotes(A) discovers C first.
        # C's reply_root is B, so a placeholder thread rooted at B is created.
        b = make_post_view("bob", "2", quote_count=0)
        c = make_post_view(
            "carol", "3",
            reply_parent=at_uri("bob", "2"),
            reply_root=at_uri("bob", "2"),
            embed_uri=at_uri("alice", "1"),
            quote_count=0,
        )

        client = MockClient()
        client.add_thread(at_uri("alice", "1"), make_thread_view(a))
        # When we fetch C's thread, we get B (root) + C (reply)
        client.add_thread(
            at_uri("carol", "3"),
            make_thread_view(c, parent=make_thread_view(b)),
        )
        client.add_thread(
            at_uri("bob", "2"),
            make_thread_view(b, replies=[make_thread_view(c)]),
        )
        client.add_quotes(at_uri("alice", "1"), [c])

        web = await crawl(client, at_uri("alice", "1"))

        assert web.thread_count == 2  # T_A and T_B (no orphan placeholder)
        # B's thread should contain both B and C
        b_thread = web.threads.get(at_uri("bob", "2"))
        assert b_thread is not None
        assert at_uri("bob", "2") in b_thread.posts
        assert at_uri("carol", "3") in b_thread.posts

    async def test_engagement_count_updates(self):
        """B8: Re-crawl updates engagement counts but preserves text."""
        existing = ContextWeb(
            root_uri=at_uri("alice", "1"),
            crawled_at="2026-01-01T00:00:00Z",
        )
        existing.add_thread(Thread(
            root_uri=at_uri("alice", "1"),
            posts={
                at_uri("alice", "1"): Post(
                    uri=at_uri("alice", "1"),
                    cid="cid-alice-1",
                    author=Author(did="did:plc:alice", handle="alice.bsky.social"),
                    text="Original text",
                    created_at="2026-01-01T00:00:00Z",
                    like_count=10,
                    quote_count=0,
                )
            },
        ))

        # API returns higher counts
        a = make_post_view("alice", "1", "Different text from API", like_count=50, reply_count=8, quote_count=0)
        client = MockClient()
        client.add_thread(at_uri("alice", "1"), make_thread_view(a))

        web = await crawl(client, at_uri("alice", "1"), existing=existing)

        post = web.nodes[at_uri("alice", "1")]
        assert post.like_count == 50  # updated
        assert post.reply_count == 8  # updated
        assert post.text == "Original text"  # preserved (not replaced)

    async def test_root_uri_normalization(self):
        """B9: Handle-based start URI is normalized to DID-based."""
        handle_uri = "at://alice.bsky.social/app.bsky.feed.post/1"
        did_uri = at_uri("alice", "1")

        a = make_post_view("alice", "1", quote_count=0)
        client = MockClient()
        client.add_thread(handle_uri, make_thread_view(a))

        web = await crawl(client, handle_uri)

        # The rkey "1" matches, so root_uri is normalized to the DID form
        assert web.root_uri == did_uri

    async def test_paginated_quotes(self):
        """B10: getQuotes paginates correctly across multiple pages."""
        a = make_post_view("alice", "1", quote_count=3)
        quoters = [
            make_post_view(f"q{i}", str(i + 10), embed_uri=at_uri("alice", "1"), quote_count=0)
            for i in range(3)
        ]

        client = MockClient()
        client.add_thread(at_uri("alice", "1"), make_thread_view(a))
        for q in quoters:
            client.add_thread(q.uri, make_thread_view(q))
        client.add_quotes(at_uri("alice", "1"), quoters, page_size=2)

        web = await crawl(client, at_uri("alice", "1"))

        # All 3 quoters collected
        assert web.node_count == 4  # A + 3 quoters
        # Two getQuotes calls (page 0 with 2 items + page 1 with 1 item)
        quote_calls = client.calls("get_quotes")
        assert len(quote_calls) == 2

    async def test_quote_edge_dedup(self):
        """B11: Same quote found via embed_uri AND getQuotes → deduplicated."""
        a = make_post_view("alice", "1", quote_count=1)
        b = make_post_view("bob", "2", embed_uri=at_uri("alice", "1"), quote_count=0)

        client = MockClient()
        client.add_thread(at_uri("alice", "1"), make_thread_view(a))
        client.add_thread(at_uri("bob", "2"), make_thread_view(b))
        # B is discovered both via _fetch_thread (embed_uri) and _fetch_quotes
        client.add_quotes(at_uri("alice", "1"), [b])

        web = await crawl(client, at_uri("alice", "1"))

        # After dedup, exactly 1 quote edge
        assert len(web.quote_edges) == 1
        assert web.quote_edges[0].source == at_uri("alice", "1")
        assert web.quote_edges[0].target == at_uri("bob", "2")

    async def test_progress_callback(self):
        """B12: progress_callback receives (node_count, edge_count, thread_count) calls."""
        a = make_post_view("alice", "1", quote_count=0)
        client = MockClient()
        client.add_thread(at_uri("alice", "1"), make_thread_view(a))

        progress_calls: list[tuple[int, int, int]] = []

        def on_progress(nodes: int, edges: int, threads: int) -> None:
            progress_calls.append((nodes, edges, threads))

        web = await crawl(client, at_uri("alice", "1"), progress_callback=on_progress)

        assert len(progress_calls) >= 1
        assert progress_calls[-1][0] == web.node_count
        assert progress_calls[-1][2] == web.thread_count


# ===================================================================
# C. Error Handling Tests
# ===================================================================


class TestErrorHandling:
    """Test graceful handling of API failures and edge cases."""

    async def test_thread_fetch_failure(self):
        """C1: getPostThread raises — empty web, no crash."""
        client = MockClient()
        # Don't register any thread — mock will raise

        web = await crawl(client, at_uri("alice", "1"))

        assert web.node_count == 0

    async def test_quotes_fetch_failure(self):
        """C2: getQuotes raises — thread preserved, no quotes, no crash."""
        a = make_post_view("alice", "1", quote_count=5)
        client = MockClient()
        client.add_thread(at_uri("alice", "1"), make_thread_view(a))
        client.set_quote_error(at_uri("alice", "1"), Exception("server error"))

        web = await crawl(client, at_uri("alice", "1"))

        assert web.node_count == 1  # A is preserved
        assert len(web.quote_edges) == 0  # no quotes discovered

    async def test_retry_on_rate_limit(self):
        """C3: _retry retries on 429 errors then succeeds."""
        call_count = 0
        rate_limit_resp = types.SimpleNamespace(status_code=429, headers={})

        async def flaky(**_kwargs):
            nonlocal call_count
            call_count += 1
            if call_count < 3:
                raise RequestException(response=rate_limit_resp)
            return "success"

        result = await _retry(flaky)

        assert result == "success"
        assert call_count == 3

    async def test_retry_on_network_error(self):
        """C3b: _retry retries on network errors then succeeds."""
        call_count = 0

        async def flaky(**_kwargs):
            nonlocal call_count
            call_count += 1
            if call_count < 2:
                raise NetworkError()
            return "success"

        result = await _retry(flaky)

        assert result == "success"
        assert call_count == 2

    async def test_retry_exhaustion_raises(self):
        """C4: Permanent non-transient error raises after first attempt."""
        async def always_fails(**_kwargs):
            raise ValueError("permanent error")

        with pytest.raises(ValueError, match="permanent"):
            await _retry(always_fails)

    async def test_timeout_stops_crawl(self):
        """C5: Tiny timeout causes early termination."""
        # Build enough data that the crawl would take multiple iterations
        a = make_post_view("alice", "1", quote_count=1)
        b = make_post_view("bob", "2", embed_uri=at_uri("alice", "1"), quote_count=1)
        c = make_post_view("carol", "3", embed_uri=at_uri("bob", "2"), quote_count=0)

        client = MockClient()
        client.add_thread(at_uri("alice", "1"), make_thread_view(a))
        client.add_thread(at_uri("bob", "2"), make_thread_view(b))
        client.add_thread(at_uri("carol", "3"), make_thread_view(c))
        client.add_quotes(at_uri("alice", "1"), [b])
        client.add_quotes(at_uri("bob", "2"), [c])

        # timeout=0 means deadline is immediately in the past after first fetch
        web = await crawl(client, at_uri("alice", "1"), timeout=0.0)

        # Should get some posts but not all
        assert web.node_count < 3

    async def test_not_found_thread_response(self):
        """C6: getPostThread returns NotFoundPost as top-level → empty result."""
        client = MockClient()
        client.add_thread(at_uri("alice", "1"), make_not_found(at_uri("alice", "1")))

        web = await crawl(client, at_uri("alice", "1"))

        assert web.node_count == 0

    async def test_termination_log_timeout(self, caplog):
        """C7: Crawl logs 'timeout' when deadline is exceeded."""
        a = make_post_view("alice", "1", quote_count=1)
        b = make_post_view("bob", "2", embed_uri=at_uri("alice", "1"), quote_count=0)

        client = MockClient()
        client.add_thread(at_uri("alice", "1"), make_thread_view(a))
        client.add_thread(at_uri("bob", "2"), make_thread_view(b))
        client.add_quotes(at_uri("alice", "1"), [b])

        with caplog.at_level("INFO", logger="bsky_context.crawler"):
            await crawl(client, at_uri("alice", "1"), timeout=0.0)

        assert any("timeout" in msg.lower() for msg in caplog.messages)

    async def test_termination_log_max_nodes(self, caplog):
        """C8: Crawl logs 'max_nodes' when node limit is reached."""
        a = make_post_view("alice", "1", quote_count=1)
        b = make_post_view("bob", "2", embed_uri=at_uri("alice", "1"), quote_count=1)
        c = make_post_view("carol", "3", embed_uri=at_uri("bob", "2"), quote_count=0)

        client = MockClient()
        client.add_thread(at_uri("alice", "1"), make_thread_view(a))
        client.add_thread(at_uri("bob", "2"), make_thread_view(b))
        client.add_thread(at_uri("carol", "3"), make_thread_view(c))
        client.add_quotes(at_uri("alice", "1"), [b])
        client.add_quotes(at_uri("bob", "2"), [c])

        with caplog.at_level("INFO", logger="bsky_context.crawler"):
            await crawl(client, at_uri("alice", "1"), max_nodes=1)

        assert any("max_nodes" in msg.lower() for msg in caplog.messages)

    async def test_termination_log_complete(self, caplog):
        """C9: Crawl logs 'fully explored' when graph is exhausted."""
        a = make_post_view("alice", "1", quote_count=0)

        client = MockClient()
        client.add_thread(at_uri("alice", "1"), make_thread_view(a))

        with caplog.at_level("INFO", logger="bsky_context.crawler"):
            await crawl(client, at_uri("alice", "1"))

        assert any("fully explored" in msg.lower() for msg in caplog.messages)

    async def test_termination_log_max_depth_still_completes(self, caplog):
        """C10: max_depth items are skipped, crawl logs 'fully explored'."""
        a = make_post_view("alice", "1", quote_count=1)
        b = make_post_view("bob", "2", embed_uri=at_uri("alice", "1"), quote_count=1)
        c = make_post_view("carol", "3", embed_uri=at_uri("bob", "2"), quote_count=0)

        client = MockClient()
        client.add_thread(at_uri("alice", "1"), make_thread_view(a))
        client.add_thread(at_uri("bob", "2"), make_thread_view(b))
        client.add_thread(at_uri("carol", "3"), make_thread_view(c))
        client.add_quotes(at_uri("alice", "1"), [b])
        client.add_quotes(at_uri("bob", "2"), [c])

        with caplog.at_level("INFO", logger="bsky_context.crawler"):
            await crawl(client, at_uri("alice", "1"), max_depth=0)

        # Depth-exceeded items are consumed from queue, so crawl ends as "fully explored"
        assert any("fully explored" in msg.lower() for msg in caplog.messages)


# ===================================================================
# D. Edge Cases from Real Data Patterns
# ===================================================================


class TestEdgeCases:
    """Test patterns observed in a real 10K-post Bluesky crawl."""

    async def test_heavily_quoted_post(self):
        """D1: A single post quoted 10 times → 11 threads, 10 quote edges."""
        a = make_post_view("alice", "1", quote_count=10)
        quoters = [
            make_post_view(f"q{i}", str(i + 10), embed_uri=at_uri("alice", "1"), quote_count=0)
            for i in range(10)
        ]

        client = MockClient()
        client.add_thread(at_uri("alice", "1"), make_thread_view(a))
        for q in quoters:
            client.add_thread(q.uri, make_thread_view(q))
        client.add_quotes(at_uri("alice", "1"), quoters)

        web = await crawl(client, at_uri("alice", "1"))

        assert web.thread_count == 11
        assert len(web.quote_edges) == 10

    async def test_diamond_quote(self):
        """D2: B and C both quote A. A's thread fetched only once."""
        a = make_post_view("alice", "1", quote_count=2)
        b = make_post_view("bob", "2", embed_uri=at_uri("alice", "1"), quote_count=0)
        c = make_post_view("carol", "3", embed_uri=at_uri("alice", "1"), quote_count=0)

        client = MockClient()
        client.add_thread(at_uri("alice", "1"), make_thread_view(a))
        client.add_thread(at_uri("bob", "2"), make_thread_view(b))
        client.add_thread(at_uri("carol", "3"), make_thread_view(c))
        client.add_quotes(at_uri("alice", "1"), [b, c])

        web = await crawl(client, at_uri("alice", "1"))

        assert web.node_count == 3
        assert web.thread_count == 3
        assert len(web.quote_edges) == 2
        # A's thread fetched exactly once
        a_calls = [u for u in client.call_uris("get_post_thread") if u == at_uri("alice", "1")]
        assert len(a_calls) == 1

    async def test_record_with_media_embed(self):
        """D3: embed_type 'recordWithMedia' is still detected as a quote."""
        a = make_post_view("alice", "1", quote_count=0)
        b = make_post_view(
            "bob", "2",
            embed_uri=at_uri("alice", "1"),
            embed_type="app.bsky.embed.recordWithMedia",
            quote_count=0,
        )

        client = MockClient()
        client.add_thread(at_uri("alice", "1"), make_thread_view(a))
        # Start from B — _fetch_thread discovers B's embed_uri pointing to A
        client.add_thread(at_uri("bob", "2"), make_thread_view(b))

        web = await crawl(client, at_uri("bob", "2"))

        # A gets queued and fetched via embed_uri discovery
        assert web.node_count == 2
        assert len(web.quote_edges) >= 1
        post_b = web.nodes[at_uri("bob", "2")]
        assert post_b.embed_type == "app.bsky.embed.recordWithMedia"

    async def test_recrawl_preserves_and_adds_threads(self):
        """D4: Re-crawl preserves existing threads and adds newly discovered ones."""
        existing = ContextWeb(
            root_uri=at_uri("alice", "1"),
            crawled_at="2026-01-01T00:00:00Z",
        )
        # Existing: thread T1 with A and B
        existing.add_thread(Thread(
            root_uri=at_uri("alice", "1"),
            posts={
                at_uri("alice", "1"): Post(
                    uri=at_uri("alice", "1"),
                    cid="cid-alice-1",
                    author=Author(did="did:plc:alice", handle="alice.bsky.social"),
                    text="Root post",
                    created_at="2026-01-01T00:00:00Z",
                    quote_count=0,  # was 0, now 1
                ),
                at_uri("bob", "2"): Post(
                    uri=at_uri("bob", "2"),
                    cid="cid-bob-2",
                    author=Author(did="did:plc:bob", handle="bob.bsky.social"),
                    text="Reply",
                    created_at="2026-01-01T00:01:00Z",
                    reply_parent=at_uri("alice", "1"),
                    reply_root=at_uri("alice", "1"),
                    quote_count=0,
                ),
            },
        ))

        # API now returns A with quote_count=1 (new quote!)
        a = make_post_view("alice", "1", quote_count=1)
        b = make_post_view(
            "bob", "2",
            reply_parent=at_uri("alice", "1"),
            reply_root=at_uri("alice", "1"),
            quote_count=0,
        )
        c = make_post_view("carol", "3", embed_uri=at_uri("alice", "1"), quote_count=0)

        client = MockClient()
        client.add_thread(
            at_uri("alice", "1"),
            make_thread_view(a, replies=[make_thread_view(b)]),
        )
        client.add_thread(at_uri("carol", "3"), make_thread_view(c))
        client.add_quotes(at_uri("alice", "1"), [c])

        web = await crawl(client, at_uri("alice", "1"), existing=existing)

        assert web.thread_count == 2  # T1 preserved + T_carol added
        assert at_uri("alice", "1") in web.threads
        assert web.node_count == 3  # A, B, C
        # Old thread still has both posts
        t1 = web.threads[at_uri("alice", "1")]
        assert at_uri("alice", "1") in t1.posts
        assert at_uri("bob", "2") in t1.posts


# ===================================================================
# E. Facet Edge Detection Tests
# ===================================================================


class TestFacetEdges:
    """Test detection of post references in link facets."""

    async def test_link_facet_creates_quote_edge(self):
        """E1: A link facet pointing to a post creates a quote edge and queues the target."""
        # Use handle-based URI as the facet link (this is what real posts contain)
        handle_uri = "at://carol.bsky.social/app.bsky.feed.post/5"
        a = make_post_view(
            "alice", "1",
            facets=[make_link_facet("https://bsky.app/profile/carol.bsky.social/post/5")],
            quote_count=0,
        )
        target = make_post_view("carol", "5", quote_count=0)

        client = MockClient()
        client.add_thread(at_uri("alice", "1"), make_thread_view(a))
        # Register under the handle-based URI (what PostRef.from_str produces)
        client.add_thread(handle_uri, make_thread_view(target))

        web = await crawl(client, at_uri("alice", "1"))

        assert web.node_count == 2
        assert len(web.quote_edges) >= 1
        edge_targets = {qe.target for qe in web.quote_edges}
        assert at_uri("alice", "1") in edge_targets

    async def test_link_facet_at_uri(self):
        """E2: A link facet with an AT URI (not bsky.app URL) also works."""
        target_uri = at_uri("carol", "5")
        a = make_post_view(
            "alice", "1",
            facets=[make_link_facet(target_uri)],
            quote_count=0,
        )
        target = make_post_view("carol", "5", quote_count=0)

        client = MockClient()
        client.add_thread(at_uri("alice", "1"), make_thread_view(a))
        client.add_thread(target_uri, make_thread_view(target))

        web = await crawl(client, at_uri("alice", "1"))

        assert web.node_count == 2
        assert len(web.quote_edges) >= 1

    async def test_link_facet_skips_non_post_urls(self):
        """E3: Link facets pointing to non-post URLs are ignored."""
        a = make_post_view(
            "alice", "1",
            facets=[make_link_facet("https://example.com/some-page")],
            quote_count=0,
        )

        client = MockClient()
        client.add_thread(at_uri("alice", "1"), make_thread_view(a))

        web = await crawl(client, at_uri("alice", "1"))

        assert web.node_count == 1
        assert len(web.quote_edges) == 0

    async def test_link_facet_deduped_with_embed(self):
        """E4: Link facet pointing to same post as embed_uri doesn't create duplicate edge."""
        target_uri = at_uri("carol", "5")
        a = make_post_view(
            "alice", "1",
            embed_uri=target_uri,
            facets=[make_link_facet(target_uri)],
            quote_count=0,
        )
        target = make_post_view("carol", "5", quote_count=0)

        client = MockClient()
        client.add_thread(at_uri("alice", "1"), make_thread_view(a))
        client.add_thread(target_uri, make_thread_view(target))

        web = await crawl(client, at_uri("alice", "1"))

        # Only 1 edge (from embed), not 2
        assert len(web.quote_edges) == 1


# ===================================================================
# F. Unknown Facet Type Tests
# ===================================================================


class TestUnknownFacets:
    """Test that unknown facet types are preserved rather than dropped."""

    async def test_unknown_facet_type_preserved(self):
        """F1: A facet with an unrecognized py_type is preserved with its type string."""
        from bsky_context.crawler import _extract_facets

        record = types.SimpleNamespace(
            facets=[
                types.SimpleNamespace(
                    index=types.SimpleNamespace(byte_start=0, byte_end=5),
                    features=[
                        types.SimpleNamespace(py_type="app.bsky.richtext.facet#futureType"),
                    ],
                ),
            ],
        )

        result = _extract_facets(record)

        assert len(result) == 1
        assert len(result[0]["features"]) == 1
        assert result[0]["features"][0]["type"] == "app.bsky.richtext.facet#futureType"


