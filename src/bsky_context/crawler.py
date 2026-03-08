"""Async crawler for Bluesky context webs."""

from __future__ import annotations

import asyncio
import logging
import re
import time
from datetime import datetime, timezone
from typing import Any, Callable

from atproto import AsyncClient
from atproto_client.exceptions import (
    InvokeTimeoutError,
    NetworkError,
    RequestException,
)

from bsky_context.models import Author, ContextWeb, Post, QuoteEdge, Thread
from bsky_context.uri import PostRef

logger = logging.getLogger(__name__)

MAX_RETRIES = 5
BASE_DELAY = 1.0

_AT_URI_RE = re.compile(r"^at://([^/]+)/([^/]+)/([^/]+)$")


# ===================================================================
# Public API
# ===================================================================


async def crawl(
    client: AsyncClient,
    start_uri: str,
    *,
    max_nodes: int = 2000,
    max_depth: int | None = None,
    timeout: float = 300.0,
    concurrency: int = 2,
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
        concurrency: Maximum number of concurrent API requests.
        existing: Optional existing ContextWeb to merge into. When provided,
            the crawler skips getQuotes calls for posts whose quote_count
            hasn't changed, saving API calls.
        progress_callback: Optional callable(node_count, edge_count, thread_count).
    """
    crawler = Crawler(
        client,
        max_nodes=max_nodes,
        max_depth=max_depth,
        timeout=timeout,
        concurrency=concurrency,
        progress_callback=progress_callback,
    )
    return await crawler.crawl(start_uri, existing=existing)


# ===================================================================
# Crawler class
# ===================================================================


class Crawler:
    """Thread-level BFS crawler for Bluesky context webs.

    Groups crawl state (web, queue, visited sets, handle map) as instance
    attributes so methods don't need to thread them as parameters.
    """

    def __init__(
        self,
        client: AsyncClient,
        *,
        max_nodes: int = 2000,
        max_depth: int | None = None,
        timeout: float = 300.0,
        concurrency: int = 2,
        progress_callback: Callable[[int, int, int], None] | None = None,
    ) -> None:
        self.client = client
        self.max_nodes = max_nodes
        self.max_depth = max_depth
        self.timeout = timeout
        self.concurrency = concurrency
        self.progress_callback = progress_callback

        # Per-crawl state, initialized in crawl()
        self.web: ContextWeb = None  # type: ignore[assignment]
        self.queue: asyncio.Queue[tuple[str, int]] = asyncio.Queue()
        self.visited_threads: set[str] = set()
        self.visited_quotes: set[str] = set()
        self._enqueued: set[str] = set()
        self.old_edge_counts: dict[str, int] = {}
        self.handle_to_did: dict[str, str] = {}
        self.deadline: float = 0.0
        self._sem: asyncio.Semaphore = asyncio.Semaphore(concurrency)
        self._stop: asyncio.Event = asyncio.Event()
        self._rate_ok: asyncio.Event = asyncio.Event()
        self._rate_ok.set()  # starts open

    async def crawl(
        self,
        start_uri: str,
        *,
        existing: ContextWeb | None = None,
    ) -> ContextWeb:
        """Run the BFS crawl and return the completed ContextWeb."""
        # Initialize per-crawl state
        self.old_edge_counts = {}
        if existing is not None:
            for qe in existing.quote_edges:
                self.old_edge_counts[qe.source] = (
                    self.old_edge_counts.get(qe.source, 0) + 1
                )
            self.web = existing
            self.web.crawled_at = datetime.now(timezone.utc).isoformat()
            # Seed handle map from existing posts
            for post in existing.iter_posts():
                self._register_post(post)
        else:
            self.web = ContextWeb(
                root_uri=start_uri,
                crawled_at=datetime.now(timezone.utc).isoformat(),
            )

        self.queue = asyncio.Queue()
        self.visited_threads = set()
        self.visited_quotes = set()
        self._enqueued = set()
        self.handle_to_did = {}
        self.deadline = time.monotonic() + self.timeout
        self._sem = asyncio.Semaphore(self.concurrency)
        self._stop = asyncio.Event()
        self._rate_ok = asyncio.Event()
        self._rate_ok.set()

        self._enqueue(start_uri, 0)

        # Launch worker pool
        workers = [
            asyncio.create_task(self._worker())
            for _ in range(self.concurrency)
        ]
        await self.queue.join()
        for w in workers:
            w.cancel()
        await asyncio.gather(*workers, return_exceptions=True)

        # Log why the crawl stopped
        if time.monotonic() > self.deadline:
            logger.info("Crawl stopped: timeout (%.0fs limit)", self.timeout)
        elif self.web.node_count >= self.max_nodes:
            logger.info("Crawl stopped: reached max_nodes (%d)", self.max_nodes)
        else:
            logger.info(
                "Crawl complete: graph fully explored (%d posts)",
                self.web.node_count,
            )

        # Resolve any remaining handle-based URIs in edges now that
        # handle_to_did is fully populated from the crawl
        for qe in self.web.quote_edges:
            qe.source = self._resolve_uri(qe.source)
            qe.target = self._resolve_uri(qe.target)

        self.web.normalize_quote_edges()

        # Normalize root_uri to canonical DID form
        resolved = self._resolve_uri(start_uri)
        if self.web.has_post(resolved):
            self.web.root_uri = resolved
        elif self.web.has_post(start_uri):
            self.web.root_uri = self.web.get_post(start_uri).uri

        return self.web

    # ---------------------------------------------------------------
    # Worker and queue management
    # ---------------------------------------------------------------

    def _enqueue(self, uri: str, depth: int) -> None:
        """Add a URI to the work queue if not already enqueued."""
        if uri not in self._enqueued:
            self._enqueued.add(uri)
            self.queue.put_nowait((uri, depth))

    def _should_stop(self) -> bool:
        return (
            self.web.node_count >= self.max_nodes
            or time.monotonic() > self.deadline
            or self._stop.is_set()
        )

    async def _worker(self) -> None:
        """BFS worker: fetch threads and their quotes from the queue."""
        while True:
            uri, depth = await self.queue.get()
            try:
                # Skip work if we should stop, but still call task_done
                if self._should_stop():
                    continue

                if self.max_depth is not None and depth > self.max_depth:
                    continue

                known_root = self.web.thread_root_for(uri)
                if known_root and known_root in self.visited_threads:
                    continue

                actual_root = await self._fetch_thread(uri, depth)
                if actual_root:
                    self.visited_threads.add(actual_root)

                # Fetch quotes for all posts we haven't checked yet
                await self._fetch_quotes_for_pending(depth)

                if self.progress_callback:
                    self.progress_callback(
                        self.web.node_count,
                        self.web.edge_count,
                        self.web.thread_count,
                    )
            finally:
                self.queue.task_done()

    async def _fetch_quotes_for_pending(self, depth: int) -> None:
        """Fetch quotes for all unchecked posts currently in the web."""
        all_posts = self.web.nodes
        to_check = [u for u in all_posts if u not in self.visited_quotes]
        for post_uri in to_check:
            if self._should_stop():
                break
            self.visited_quotes.add(post_uri)

            current_quote_count = all_posts[post_uri].quote_count
            if current_quote_count == 0:
                continue

            if post_uri in self.old_edge_counts:
                if current_quote_count <= self.old_edge_counts[post_uri]:
                    continue

            await self._fetch_quotes(post_uri, depth)

    # ---------------------------------------------------------------
    # Handle → DID resolution
    # ---------------------------------------------------------------

    def _register_post(self, post: Post) -> None:
        """Record a handle→DID mapping from a post's author."""
        self.handle_to_did[post.author.handle] = post.author.did

    def _resolve_uri(self, uri: str) -> str:
        """Normalize a handle-based AT URI to its canonical DID-based form.

        If the URI already uses a DID, or the handle isn't in our map,
        returns the URI unchanged.
        """
        if uri in self.web._post_index:
            return uri
        m = _AT_URI_RE.match(uri)
        if not m:
            return uri
        authority, collection, rkey = m.group(1), m.group(2), m.group(3)
        if authority.startswith("did:"):
            return uri  # already canonical
        did = self.handle_to_did.get(authority)
        if not did:
            return uri  # unknown handle, can't resolve
        canonical = f"at://{did}/{collection}/{rkey}"
        if canonical in self.web._post_index:
            return canonical
        return uri  # DID-based URI not in web either

    # ---------------------------------------------------------------
    # Thread fetching
    # ---------------------------------------------------------------

    async def _fetch_thread(self, uri: str, depth: int) -> str | None:
        """Fetch a post's thread and ingest as a Thread object.

        Returns the thread root URI, or None on failure.
        """
        try:
            await self._rate_ok.wait()
            async with self._sem:
                resp = await _retry(
                    self.client.app.bsky.feed.get_post_thread,
                    params={"uri": uri, "depth": 1000, "parentHeight": 1000},
                    rate_event=self._rate_ok,
                )
        except Exception as e:
            logger.warning("Failed to fetch thread for %s: %s", uri, e)
            return None
        if resp is None:
            return None

        # Walk the tree to collect all posts
        posts: dict[str, Post] = {}
        _walk_thread_node(resp.thread, posts)

        if not posts:
            return None

        # Register all authors for handle→DID resolution
        for post in posts.values():
            self._register_post(post)

        # Find thread root: topmost ancestor in the response
        thread_root_uri = _find_response_root(resp.thread)
        if not thread_root_uri:
            for p in posts.values():
                if not p.reply_parent or p.reply_parent not in posts:
                    thread_root_uri = p.uri
                    break
            if not thread_root_uri:
                thread_root_uri = uri

        # Check if any collected post already belongs to an existing thread
        existing_root = None
        for p_uri in posts:
            root = self.web.thread_root_for(p_uri)
            if root is not None:
                existing_root = root
                break

        if existing_root and existing_root != thread_root_uri:
            old_thread = self.web.remove_thread(existing_root)
            if thread_root_uri not in self.web.threads:
                self.web.add_thread(Thread(root_uri=thread_root_uri))
            for p_uri, p in old_thread.posts.items():
                if not self.web.has_post(p_uri):
                    self.web.add_post(thread_root_uri, p)
            for qe in self.web.quote_edges:
                if qe.source_thread == existing_root:
                    qe.source_thread = thread_root_uri
                if qe.target_thread == existing_root:
                    qe.target_thread = thread_root_uri
        elif thread_root_uri not in self.web.threads:
            self.web.add_thread(Thread(root_uri=thread_root_uri))

        # Add/update posts in thread
        for p_uri, post in posts.items():
            if self.web.has_post(p_uri):
                existing_post = self.web.get_post(p_uri)
                existing_post.like_count = post.like_count
                existing_post.reply_count = post.reply_count
                existing_post.repost_count = post.repost_count
                existing_post.quote_count = post.quote_count
            else:
                self.web.add_post(thread_root_uri, post)

        # Create quote edges and queue quoted post targets
        for post in posts.values():
            if post.embed_uri:
                resolved = self._resolve_uri(post.embed_uri)
                target_thread_root = (
                    self.web.thread_root_for(resolved) or resolved
                )
                self.web.quote_edges.append(
                    QuoteEdge(
                        source=resolved,
                        target=post.uri,
                        source_thread=target_thread_root,
                        target_thread=thread_root_uri,
                    )
                )
                if not self.web.thread_root_for(resolved):
                    if self.max_depth is None or depth + 1 <= self.max_depth:
                        self._enqueue(post.embed_uri, depth + 1)

            # Detect quote-like references in link facets
            for facet in post.facets:
                for feat in facet.get("features", []):
                    if feat.get("type") != "link":
                        continue
                    facet_uri = _resolve_facet_link(feat.get("uri", ""))
                    if not facet_uri:
                        continue
                    resolved = self._resolve_uri(facet_uri)
                    if resolved == self._resolve_uri(post.embed_uri or ""):
                        continue  # already handled as embed quote
                    if resolved == post.uri:
                        continue  # self-reference
                    target_thread = (
                        self.web.thread_root_for(resolved) or resolved
                    )
                    self.web.quote_edges.append(
                        QuoteEdge(
                            source=resolved,
                            target=post.uri,
                            source_thread=target_thread,
                            target_thread=thread_root_uri,
                        )
                    )
                    if not self.web.thread_root_for(resolved):
                        if (
                            self.max_depth is None
                            or depth + 1 <= self.max_depth
                        ):
                            self._enqueue(facet_uri, depth + 1)

        return thread_root_uri

    # ---------------------------------------------------------------
    # Quote fetching
    # ---------------------------------------------------------------

    async def _fetch_quotes(self, uri: str, depth: int) -> None:
        """Fetch all posts that quote the given URI, paginating through results."""
        source_thread = self.web.thread_root_for(uri) or uri

        cursor = None
        while True:
            try:
                params: dict[str, Any] = {"uri": uri, "limit": 100}
                if cursor:
                    params["cursor"] = cursor
                await self._rate_ok.wait()
                async with self._sem:
                    resp = await _retry(
                        self.client.app.bsky.feed.get_quotes,
                        params=params,
                        rate_event=self._rate_ok,
                    )
            except Exception as e:
                logger.warning("Failed to fetch quotes for %s: %s", uri, e)
                break
            if resp is None:
                break

            for post_view in resp.posts or []:
                post = _extract_post(post_view)
                self._register_post(post)
                if not self.web.has_post(post.uri):
                    target_thread_root = (
                        post.reply_root if post.reply_root else post.uri
                    )

                    if target_thread_root not in self.web.threads:
                        self.web.add_thread(
                            Thread(root_uri=target_thread_root)
                        )
                    self.web.add_post(target_thread_root, post)

                    self.web.quote_edges.append(
                        QuoteEdge(
                            source=uri,
                            target=post.uri,
                            source_thread=source_thread,
                            target_thread=target_thread_root,
                        )
                    )

                    if (
                        self.max_depth is None
                        or depth + 1 <= self.max_depth
                    ):
                        # Enqueue the thread root when known (from reply_root),
                        # otherwise the post URI itself
                        self._enqueue(
                            post.reply_root or post.uri, depth + 1,
                        )

            cursor = getattr(resp, "cursor", None)
            if not cursor:
                break


# ===================================================================
# Module-level helpers (pure functions, no shared state)
# ===================================================================


async def _retry(coro_factory, *args, rate_event: asyncio.Event | None = None, **kwargs):
    """Retry an async call with exponential backoff on rate limit or transient errors."""
    for attempt in range(MAX_RETRIES):
        try:
            return await coro_factory(*args, **kwargs)
        except RequestException as e:
            status = getattr(
                getattr(e, "response", None), "status_code", None
            )
            if status == 429:
                headers = (
                    getattr(getattr(e, "response", None), "headers", {})
                    or {}
                )
                retry_after = headers.get("retry-after") or headers.get(
                    "Retry-After"
                )
                if retry_after and retry_after.isdigit():
                    delay = int(retry_after)
                else:
                    delay = BASE_DELAY * (2**attempt)
                logger.info(
                    "Rate limited (429), pausing all requests for %.1fs (attempt %d/%d)",
                    delay,
                    attempt + 1,
                    MAX_RETRIES,
                )
                if rate_event is not None:
                    rate_event.clear()  # block all other workers
                await asyncio.sleep(delay)
                if rate_event is not None:
                    rate_event.set()  # release all workers
            else:
                raise
        except InvokeTimeoutError:
            if attempt < MAX_RETRIES - 1:
                logger.info(
                    "Timeout, retrying (attempt %d/%d)",
                    attempt + 1,
                    MAX_RETRIES,
                )
                await asyncio.sleep(BASE_DELAY)
            else:
                raise
        except NetworkError:
            if attempt < MAX_RETRIES - 1:
                logger.info(
                    "Network error, retrying (attempt %d/%d)",
                    attempt + 1,
                    MAX_RETRIES,
                )
                await asyncio.sleep(BASE_DELAY)
            else:
                raise
    return None


def _walk_thread_node(
    node: Any,
    posts: dict[str, Post],
) -> None:
    """Recursively walk a ThreadViewPost tree, collecting posts."""
    if not hasattr(node, "post"):
        return

    post = _extract_post(node.post)
    if post.uri not in posts:
        posts[post.uri] = post

    if hasattr(node, "parent") and node.parent:
        _walk_thread_node(node.parent, posts)

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

    reply_parent = None
    reply_root = None
    if hasattr(record, "reply") and record.reply:
        reply_parent = record.reply.parent.uri
        reply_root = record.reply.root.uri

    embed_type = None
    embed_uri = None
    if hasattr(record, "embed") and record.embed:
        embed = record.embed
        type_str = getattr(embed, "py_type", "") or ""
        if "record" in type_str.lower():
            embed_type = type_str
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


def _resolve_facet_link(url: str) -> str | None:
    """If url is an AT URI or bsky.app post URL, return the AT URI. Else None."""
    if not url:
        return None
    if url.startswith("at://") and "/app.bsky.feed.post/" in url:
        return url
    try:
        return PostRef.from_str(url).at_uri
    except ValueError:
        return None


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
            else:
                facet_dict["features"].append({"type": feat_type})
        result.append(facet_dict)
    return result
