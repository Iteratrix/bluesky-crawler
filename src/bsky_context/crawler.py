"""Async crawler for Bluesky context webs."""

from __future__ import annotations

import asyncio
import time
from collections import deque
from datetime import datetime, timezone
from typing import Any, Callable

from atproto import AsyncClient

from bsky_context.models import Author, ContextWeb, Edge, EdgeType, Post

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
    progress_callback: Callable[[int, int], None] | None = None,
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
        progress_callback: Optional callable(node_count, edge_count).
    """
    # Snapshot old quote counts before we start updating them
    old_quote_counts: dict[str, int] = {}
    if existing is not None:
        for uri, post in existing.nodes.items():
            old_quote_counts[uri] = post.quote_count
        web = existing
        web.crawled_at = datetime.now(timezone.utc).isoformat()
    else:
        web = ContextWeb(
            root_uri=start_uri,
            crawled_at=datetime.now(timezone.utc).isoformat(),
        )

    # BFS queue entries: (uri, depth)
    queue: deque[tuple[str, int]] = deque([(start_uri, 0)])
    visited_threads: set[str] = set()
    visited_quotes: set[str] = set()
    deadline = time.monotonic() + timeout

    while queue and web.node_count < max_nodes:
        if time.monotonic() > deadline:
            break

        uri, depth = queue.popleft()
        if max_depth is not None and depth > max_depth:
            continue

        # Fetch this post's thread
        if uri not in visited_threads:
            visited_threads.add(uri)
            await _fetch_thread(client, uri, depth, web, queue, max_depth)

        # Fetch quotes for all posts we haven't checked yet
        to_check = [u for u in web.nodes if u not in visited_quotes]
        for post_uri in to_check:
            if web.node_count >= max_nodes or time.monotonic() > deadline:
                break
            visited_quotes.add(post_uri)

            # Skip getQuotes if quote_count hasn't changed since last crawl
            if post_uri in old_quote_counts:
                current = web.nodes[post_uri].quote_count
                if current == old_quote_counts[post_uri]:
                    continue

            post_depth = _post_depth(web, post_uri, start_uri)
            await _fetch_quotes(client, post_uri, post_depth, web, queue, max_depth)

        if progress_callback:
            progress_callback(web.node_count, web.edge_count)

    web.deduplicate_edges()
    # Normalize root_uri to canonical DID form if we have it
    if start_uri in web.nodes:
        web.root_uri = web.nodes[start_uri].uri
    elif web.nodes:
        # The start URI might have been a handle-based URI; find the canonical version
        for uri, post in web.nodes.items():
            rkey = start_uri.rsplit("/", 1)[-1]
            if uri.endswith(f"/{rkey}"):
                web.root_uri = uri
                break

    return web


def _post_depth(web: ContextWeb, uri: str, start_uri: str) -> int:
    """Estimate BFS depth of a post (simple heuristic: count hops via edges)."""
    # For simplicity, default to 1 — the queue tracks actual depth for new entries
    return 1


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
) -> None:
    """Fetch a post's thread and extract nodes + edges."""
    try:
        resp = await _retry(
            client.app.bsky.feed.get_post_thread,
            params={"uri": uri, "depth": 1000, "parentHeight": 1000},
        )
    except Exception:
        return
    if resp is None:
        return

    _walk_thread_node(resp.thread, depth, web, queue, max_depth)


def _walk_thread_node(
    node: Any,
    depth: int,
    web: ContextWeb,
    queue: deque[tuple[str, int]],
    max_depth: int | None,
) -> None:
    """Recursively walk a ThreadViewPost tree."""
    # node might be NotFoundPost, BlockedPost, or other non-post types
    if not hasattr(node, "post"):
        return

    post = _extract_post(node.post)
    is_new = post.uri not in web.nodes
    if is_new:
        web.nodes[post.uri] = post
    else:
        # Update engagement counts on existing posts (these change over time)
        existing = web.nodes[post.uri]
        existing.like_count = post.like_count
        existing.reply_count = post.reply_count
        existing.repost_count = post.repost_count
        existing.quote_count = post.quote_count

    # Reply edge: parent -> this post
    if post.reply_parent:
        web.edges.append(Edge(
            source=post.reply_parent,
            target=post.uri,
            type=EdgeType.REPLY,
        ))

    # Quote edge: quoted -> this post (quoter)
    if post.embed_uri:
        web.edges.append(Edge(
            source=post.embed_uri,
            target=post.uri,
            type=EdgeType.QUOTE,
        ))
        # Queue the quoted post for crawling
        if post.embed_uri not in web.nodes:
            queue.append((post.embed_uri, depth + 1))

    # Walk parent chain (ancestors)
    if hasattr(node, "parent") and node.parent:
        _walk_thread_node(node.parent, depth, web, queue, max_depth)

    # Walk replies (descendants)
    if hasattr(node, "replies") and node.replies:
        for reply_node in node.replies:
            if max_depth is None or depth + 1 <= max_depth:
                _walk_thread_node(reply_node, depth + 1, web, queue, max_depth)


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
            if post.uri not in web.nodes:
                web.nodes[post.uri] = post
                web.edges.append(Edge(
                    source=uri,
                    target=post.uri,
                    type=EdgeType.QUOTE,
                ))
                if max_depth is None or depth + 1 <= max_depth:
                    queue.append((post.uri, depth + 1))

        cursor = getattr(resp, "cursor", None)
        if not cursor:
            break
