"""Async crawler for Bluesky context webs."""

from __future__ import annotations

import asyncio
import time
from collections import deque
from datetime import datetime, timezone
from typing import Any, Callable

from atproto import AsyncClient

from bsky_context.models import Author, ContextWeb, Post, QuoteEdge, Thread

MAX_RETRIES = 5
BASE_DELAY = 1.0


async def crawl(
    client: AsyncClient,
    start_uri: str,
    *,
    max_nodes: int = 2000,
    max_depth: int | None = None,
    timeout: float = 300.0,
    existing: ContextWeb | None = None,
    progress_callback: Callable[[int, int, int], None] | None = None,
) -> ContextWeb:
    """Crawl the full context web starting from a post URI.

    Args:
        client: Authenticated AsyncClient.
        start_uri: AT URI of the starting post.
        max_nodes: Maximum number of posts to collect.
        max_depth: Maximum BFS hop distance from start post (None = unlimited).
        timeout: Maximum wall-clock seconds for the entire crawl.
        existing: Optional existing ContextWeb to merge into. When provided,
            the crawler skips getQuotes calls for posts whose quote_count
            hasn't changed, saving API calls.
        progress_callback: Optional callable(node_count, edge_count, thread_count).
    """
    # Count existing quote edges per source, so we can detect which posts
    # had their quotes explored vs. which were discovered but not followed.
    old_edge_counts: dict[str, int] = {}
    if existing is not None:
        for qe in existing.quote_edges:
            old_edge_counts[qe.source] = old_edge_counts.get(qe.source, 0) + 1
        web = existing
        web.crawled_at = datetime.now(timezone.utc).isoformat()
    else:
        web = ContextWeb(
            root_uri=start_uri,
            crawled_at=datetime.now(timezone.utc).isoformat(),
        )

    # BFS queue entries: (uri, depth)
    queue: deque[tuple[str, int]] = deque([(start_uri, 0)])
    visited_threads: set[str] = set()  # thread root URIs we've fetched
    visited_quotes: set[str] = set()  # post URIs we've checked for quotes
    deadline = time.monotonic() + timeout

    while queue and web.node_count < max_nodes:
        if time.monotonic() > deadline:
            break

        uri, depth = queue.popleft()
        if max_depth is not None and depth > max_depth:
            continue

        # Check if this post is in a thread we've already fetched
        known_root = _known_thread_root(web, uri)
        if known_root and known_root in visited_threads:
            continue

        # Fetch this post's thread
        actual_root = await _fetch_thread(client, uri, depth, web, queue, max_depth)
        if actual_root:
            visited_threads.add(actual_root)

        # Fetch quotes for all posts we haven't checked yet
        all_posts = web.nodes
        to_check = [u for u in all_posts if u not in visited_quotes]
        for post_uri in to_check:
            if web.node_count >= max_nodes or time.monotonic() > deadline:
                break
            visited_quotes.add(post_uri)

            current_quote_count = all_posts[post_uri].quote_count
            if current_quote_count == 0:
                continue

            # Skip getQuotes if we already have edges from a previous crawl
            # and the quote_count hasn't increased
            if post_uri in old_edge_counts:
                if current_quote_count <= old_edge_counts[post_uri]:
                    continue

            await _fetch_quotes(client, post_uri, depth, web, queue, max_depth)

        if progress_callback:
            progress_callback(web.node_count, web.edge_count, web.thread_count)

    web.normalize_quote_edges()
    # Normalize root_uri to canonical DID form if we have it
    if web.has_post(start_uri):
        web.root_uri = web.get_post(start_uri).uri
    elif web.node_count > 0:
        # The start URI might have been a handle-based URI; find the canonical version
        rkey = start_uri.rsplit("/", 1)[-1]
        for uri in web._post_index:
            if uri.endswith(f"/{rkey}"):
                web.root_uri = uri
                break

    return web


def _known_thread_root(web: ContextWeb, uri: str) -> str | None:
    """If we already know which thread contains this URI, return its root."""
    return web.thread_root_for(uri)


async def _retry(coro_factory, *args, **kwargs):
    """Retry an async call with exponential backoff on rate limit or transient errors."""
    for attempt in range(MAX_RETRIES):
        try:
            return await coro_factory(*args, **kwargs)
        except Exception as e:
            err = str(e).lower()
            if "429" in err or "rate" in err or "too many" in err:
                delay = BASE_DELAY * (2 ** attempt)
                await asyncio.sleep(delay)
            elif attempt < MAX_RETRIES - 1 and ("timeout" in err or "connection" in err):
                await asyncio.sleep(BASE_DELAY)
            else:
                raise
    return None


async def _fetch_thread(
    client: AsyncClient,
    uri: str,
    depth: int,
    web: ContextWeb,
    queue: deque[tuple[str, int]],
    max_depth: int | None,
) -> str | None:
    """Fetch a post's thread and ingest as a Thread object.

    Returns the thread root URI, or None on failure.
    """
    try:
        resp = await _retry(
            client.app.bsky.feed.get_post_thread,
            params={"uri": uri, "depth": 1000, "parentHeight": 1000},
        )
    except Exception:
        return None
    if resp is None:
        return None

    # Walk the tree to collect all posts
    posts: dict[str, Post] = {}
    _walk_thread_node(resp.thread, posts)

    if not posts:
        return None

    # Find thread root: topmost ancestor in the response
    thread_root_uri = _find_response_root(resp.thread)
    if not thread_root_uri:
        # Fallback: find post with no reply_parent in our collected set
        for p in posts.values():
            if not p.reply_parent or p.reply_parent not in posts:
                thread_root_uri = p.uri
                break
        if not thread_root_uri:
            thread_root_uri = uri

    # Check if any collected post already belongs to an existing thread
    # (handles the case where a placeholder thread was created with a different root)
    existing_root = None
    for p_uri in posts:
        root = web.thread_root_for(p_uri)
        if root is not None:
            existing_root = root
            break

    if existing_root and existing_root != thread_root_uri:
        # Merge placeholder thread into the real thread
        old_thread = web.remove_thread(existing_root)
        if thread_root_uri not in web.threads:
            web.add_thread(Thread(root_uri=thread_root_uri))
        # Move posts from old placeholder
        for p_uri, p in old_thread.posts.items():
            if not web.has_post(p_uri):
                web.add_post(thread_root_uri, p)
        # Update quote edges referencing old root
        for qe in web.quote_edges:
            if qe.source_thread == existing_root:
                qe.source_thread = thread_root_uri
            if qe.target_thread == existing_root:
                qe.target_thread = thread_root_uri
    elif thread_root_uri not in web.threads:
        web.add_thread(Thread(root_uri=thread_root_uri))

    # Add/update posts in thread
    for p_uri, post in posts.items():
        if web.has_post(p_uri):
            # Update engagement counts on existing posts
            existing_post = web.get_post(p_uri)
            existing_post.like_count = post.like_count
            existing_post.reply_count = post.reply_count
            existing_post.repost_count = post.repost_count
            existing_post.quote_count = post.quote_count
        else:
            web.add_post(thread_root_uri, post)

    # Create quote edges and queue quoted post targets
    for post in posts.values():
        if post.embed_uri:
            target_thread_root = _known_thread_root(web, post.embed_uri) or post.embed_uri
            web.quote_edges.append(QuoteEdge(
                source=post.embed_uri,
                target=post.uri,
                source_thread=target_thread_root,
                target_thread=thread_root_uri,
            ))
            # Queue the quoted post for crawling if we don't have its thread yet
            known = _known_thread_root(web, post.embed_uri)
            if not known:
                if max_depth is None or depth + 1 <= max_depth:
                    queue.append((post.embed_uri, depth + 1))

    return thread_root_uri


def _walk_thread_node(
    node: Any,
    posts: dict[str, Post],
) -> None:
    """Recursively walk a ThreadViewPost tree, collecting posts."""
    # node might be NotFoundPost, BlockedPost, or other non-post types
    if not hasattr(node, "post"):
        return

    post = _extract_post(node.post)
    if post.uri not in posts:
        posts[post.uri] = post

    # Walk parent chain (ancestors)
    if hasattr(node, "parent") and node.parent:
        _walk_thread_node(node.parent, posts)

    # Walk replies (descendants)
    if hasattr(node, "replies") and node.replies:
        for reply_node in node.replies:
            _walk_thread_node(reply_node, posts)


def _find_response_root(node: Any) -> str:
    """Walk up the parent chain to find the topmost post in a getPostThread response."""
    while hasattr(node, "parent") and node.parent and hasattr(node.parent, "post"):
        node = node.parent
    if hasattr(node, "post"):
        return node.post.uri
    return ""


def _extract_post(post_view: Any) -> Post:
    """Convert an atproto PostView to our Post model."""
    record = post_view.record

    # Reply refs
    reply_parent = None
    reply_root = None
    if hasattr(record, "reply") and record.reply:
        reply_parent = record.reply.parent.uri
        reply_root = record.reply.root.uri

    # Quote embed — could be embed.record or embed.recordWithMedia
    embed_type = None
    embed_uri = None
    if hasattr(record, "embed") and record.embed:
        embed = record.embed
        type_str = getattr(embed, "py_type", "") or ""
        if "record" in type_str.lower():
            embed_type = type_str
            # app.bsky.embed.record has .record.uri directly
            inner = getattr(embed, "record", None)
            if inner:
                embed_uri = getattr(inner, "uri", None)

    author = Author(
        did=post_view.author.did,
        handle=post_view.author.handle,
        display_name=getattr(post_view.author, "display_name", "") or "",
    )

    return Post(
        uri=post_view.uri,
        cid=post_view.cid,
        author=author,
        text=getattr(record, "text", "") or "",
        created_at=getattr(record, "created_at", "") or "",
        reply_parent=reply_parent,
        reply_root=reply_root,
        embed_type=embed_type,
        embed_uri=embed_uri,
        facets=_extract_facets(record),
        labels=[l.val for l in (post_view.labels or [])],
        langs=getattr(record, "langs", []) or [],
        like_count=getattr(post_view, "like_count", 0) or 0,
        reply_count=getattr(post_view, "reply_count", 0) or 0,
        repost_count=getattr(post_view, "repost_count", 0) or 0,
        quote_count=getattr(post_view, "quote_count", 0) or 0,
    )


def _extract_facets(record: Any) -> list[dict[str, Any]]:
    """Extract facets (rich text annotations) from a post record."""
    facets = getattr(record, "facets", None)
    if not facets:
        return []
    result = []
    for f in facets:
        facet_dict: dict[str, Any] = {
            "index": {
                "byteStart": f.index.byte_start,
                "byteEnd": f.index.byte_end,
            },
            "features": [],
        }
        for feat in f.features:
            feat_type = getattr(feat, "py_type", "")
            if "mention" in feat_type:
                facet_dict["features"].append({"type": "mention", "did": feat.did})
            elif "link" in feat_type:
                facet_dict["features"].append({"type": "link", "uri": feat.uri})
            elif "tag" in feat_type:
                facet_dict["features"].append({"type": "tag", "tag": feat.tag})
        result.append(facet_dict)
    return result


async def _fetch_quotes(
    client: AsyncClient,
    uri: str,
    depth: int,
    web: ContextWeb,
    queue: deque[tuple[str, int]],
    max_depth: int | None,
) -> None:
    """Fetch all posts that quote the given URI, paginating through results."""
    source_thread = _known_thread_root(web, uri) or uri

    cursor = None
    while True:
        try:
            params: dict[str, Any] = {"uri": uri, "limit": 100}
            if cursor:
                params["cursor"] = cursor
            resp = await _retry(
                client.app.bsky.feed.get_quotes,
                params=params,
            )
        except Exception:
            break
        if resp is None:
            break

        for post_view in resp.posts or []:
            post = _extract_post(post_view)
            if not web.has_post(post.uri):
                # Determine this post's thread root
                target_thread_root = post.reply_root if post.reply_root else post.uri

                # Add to appropriate thread or create placeholder
                if target_thread_root not in web.threads:
                    web.add_thread(Thread(root_uri=target_thread_root))
                web.add_post(target_thread_root, post)

                web.quote_edges.append(QuoteEdge(
                    source=uri,
                    target=post.uri,
                    source_thread=source_thread,
                    target_thread=target_thread_root,
                ))

                if max_depth is None or depth + 1 <= max_depth:
                    queue.append((post.uri, depth + 1))

        cursor = getattr(resp, "cursor", None)
        if not cursor:
            break
