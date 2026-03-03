"""Mock infrastructure for crawler tests."""

from __future__ import annotations

import types
from typing import Any

import pytest


# ---------------------------------------------------------------------------
# URI helper
# ---------------------------------------------------------------------------

def at_uri(author: str, rkey: str) -> str:
    """Build an AT URI from short names.

    at_uri("alice", "1") -> "at://did:plc:alice/app.bsky.feed.post/1"
    """
    return f"at://did:plc:{author}/app.bsky.feed.post/{rkey}"


# ---------------------------------------------------------------------------
# PostView mock builder
# ---------------------------------------------------------------------------

def make_post_view(
    author: str,
    rkey: str,
    text: str | None = None,
    *,
    reply_parent: str | None = None,
    reply_root: str | None = None,
    embed_uri: str | None = None,
    embed_type: str = "app.bsky.embed.record",
    like_count: int = 0,
    reply_count: int = 0,
    repost_count: int = 0,
    quote_count: int = 0,
    created_at: str = "2026-01-01T00:00:00Z",
) -> types.SimpleNamespace:
    """Build a PostView-shaped SimpleNamespace matching what _extract_post reads."""
    uri = at_uri(author, rkey)

    record = types.SimpleNamespace(
        text=text or f"Post {rkey} by {author}",
        created_at=created_at,
        facets=None,
        langs=[],
    )

    # Reply refs
    if reply_parent:
        record.reply = types.SimpleNamespace(
            parent=types.SimpleNamespace(uri=reply_parent),
            root=types.SimpleNamespace(uri=reply_root or reply_parent),
        )
    else:
        record.reply = None

    # Embed (quote)
    if embed_uri:
        record.embed = types.SimpleNamespace(
            py_type=embed_type,
            record=types.SimpleNamespace(uri=embed_uri),
        )
    else:
        record.embed = None

    return types.SimpleNamespace(
        uri=uri,
        cid=f"cid-{author}-{rkey}",
        author=types.SimpleNamespace(
            did=f"did:plc:{author}",
            handle=f"{author}.bsky.social",
            display_name=author.capitalize(),
        ),
        record=record,
        labels=[],
        like_count=like_count,
        reply_count=reply_count,
        repost_count=repost_count,
        quote_count=quote_count,
    )


# ---------------------------------------------------------------------------
# ThreadViewPost mock builder
# ---------------------------------------------------------------------------

def make_thread_view(
    post_view: types.SimpleNamespace,
    *,
    parent: types.SimpleNamespace | None = None,
    replies: list[types.SimpleNamespace] | None = None,
) -> types.SimpleNamespace:
    """Build a ThreadViewPost-shaped namespace."""
    return types.SimpleNamespace(
        post=post_view,
        parent=parent,
        replies=replies or [],
    )


# ---------------------------------------------------------------------------
# Error node builders (no .post attribute)
# ---------------------------------------------------------------------------

def make_not_found(uri: str) -> types.SimpleNamespace:
    """NotFoundPost — _walk_thread_node skips these (no .post attr)."""
    return types.SimpleNamespace(not_found=True, uri=uri)


def make_blocked(uri: str) -> types.SimpleNamespace:
    """BlockedPost — _walk_thread_node skips these (no .post attr)."""
    return types.SimpleNamespace(
        blocked=True,
        uri=uri,
        author=types.SimpleNamespace(did="did:plc:blocked"),
    )


# ---------------------------------------------------------------------------
# MockClient
# ---------------------------------------------------------------------------

class MockClient:
    """Fake AsyncClient that returns pre-configured thread/quote responses.

    Usage::

        client = MockClient()
        client.add_thread(at_uri("alice", "1"), make_thread_view(pv))
        client.add_quotes(at_uri("alice", "1"), [quote_pv1, quote_pv2])

        web = await crawl(client, at_uri("alice", "1"))

        # Assert on API calls made:
        thread_calls = client.calls("get_post_thread")
    """

    def __init__(self) -> None:
        self._threads: dict[str, types.SimpleNamespace] = {}
        self._quote_pages: dict[str, list[list[types.SimpleNamespace]]] = {}
        self._quote_errors: dict[str, Exception] = {}
        self._call_log: list[tuple[str, dict[str, Any]]] = []

        # Build the nested namespace chain: client.app.bsky.feed.{method}
        self.app = types.SimpleNamespace(
            bsky=types.SimpleNamespace(
                feed=types.SimpleNamespace(
                    get_post_thread=self._get_post_thread,
                    get_quotes=self._get_quotes,
                )
            )
        )

    # -- configuration methods --

    def add_thread(self, uri: str, thread_node: types.SimpleNamespace) -> None:
        """Register a getPostThread response for a URI."""
        self._threads[uri] = types.SimpleNamespace(thread=thread_node)

    def add_quotes(
        self,
        uri: str,
        posts: list[types.SimpleNamespace],
        *,
        page_size: int = 100,
    ) -> None:
        """Register getQuotes responses, auto-paginating."""
        pages: list[list[types.SimpleNamespace]] = []
        for i in range(0, len(posts), page_size):
            pages.append(posts[i : i + page_size])
        self._quote_pages[uri] = pages if pages else [[]]

    def set_quote_error(self, uri: str, error: Exception) -> None:
        """Make getQuotes raise for a specific URI."""
        self._quote_errors[uri] = error

    # -- query helpers --

    def calls(self, method: str) -> list[dict[str, Any]]:
        """Return params dicts for all calls to a given method."""
        return [params for name, params in self._call_log if name == method]

    def call_uris(self, method: str) -> list[str]:
        """Return the URIs from all calls to a given method."""
        return [params["uri"] for params in self.calls(method)]

    # -- async API methods --

    async def _get_post_thread(self, *, params: dict[str, Any]) -> types.SimpleNamespace:
        self._call_log.append(("get_post_thread", params))
        uri = params["uri"]
        if uri not in self._threads:
            raise Exception(f"Post not found: {uri}")
        return self._threads[uri]

    async def _get_quotes(self, *, params: dict[str, Any]) -> types.SimpleNamespace:
        self._call_log.append(("get_quotes", params))
        uri = params["uri"]

        if uri in self._quote_errors:
            raise self._quote_errors[uri]

        pages = self._quote_pages.get(uri)
        if not pages:
            return types.SimpleNamespace(posts=[], cursor=None)

        cursor = params.get("cursor")
        page_idx = int(cursor) if cursor else 0
        posts = pages[page_idx] if page_idx < len(pages) else []
        next_cursor = str(page_idx + 1) if page_idx + 1 < len(pages) else None

        return types.SimpleNamespace(posts=posts, cursor=next_cursor)


# ---------------------------------------------------------------------------
# Autouse fixture: eliminate asyncio.sleep delays in _retry
# ---------------------------------------------------------------------------

@pytest.fixture(autouse=True)
def _patch_crawler_sleep(monkeypatch: pytest.MonkeyPatch) -> None:
    """Replace asyncio.sleep in the crawler module with a no-op."""
    async def _instant(*_args: Any, **_kwargs: Any) -> None:
        pass

    monkeypatch.setattr("bsky_context.crawler.asyncio.sleep", _instant)
