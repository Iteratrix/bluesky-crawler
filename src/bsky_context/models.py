"""Data models for the Context Web graph."""

from __future__ import annotations

from collections.abc import Iterator
from dataclasses import dataclass, field
from typing import Any


@dataclass
class Author:
    did: str
    handle: str
    display_name: str = ""

    def to_dict(self) -> dict[str, str]:
        return {
            "did": self.did,
            "handle": self.handle,
            "display_name": self.display_name,
        }

    @classmethod
    def from_dict(cls, d: dict[str, str]) -> Author:
        return cls(
            did=d["did"],
            handle=d["handle"],
            display_name=d.get("display_name", ""),
        )


@dataclass
class Post:
    """A single post node in the context web."""

    uri: str
    cid: str
    author: Author
    text: str
    created_at: str  # ISO 8601
    reply_parent: str | None = None  # URI of parent post
    reply_root: str | None = None  # URI of thread root post
    embed_type: str | None = None  # e.g. "app.bsky.embed.record"
    embed_uri: str | None = None  # URI of quoted post
    facets: list[dict[str, Any]] = field(default_factory=list)
    labels: list[str] = field(default_factory=list)
    langs: list[str] = field(default_factory=list)
    like_count: int = 0
    reply_count: int = 0
    repost_count: int = 0
    quote_count: int = 0

    def to_dict(self) -> dict[str, Any]:
        return {
            "uri": self.uri,
            "cid": self.cid,
            "author": self.author.to_dict(),
            "text": self.text,
            "created_at": self.created_at,
            "reply_parent": self.reply_parent,
            "reply_root": self.reply_root,
            "embed_type": self.embed_type,
            "embed_uri": self.embed_uri,
            "facets": self.facets,
            "labels": self.labels,
            "langs": self.langs,
            "like_count": self.like_count,
            "reply_count": self.reply_count,
            "repost_count": self.repost_count,
            "quote_count": self.quote_count,
        }

    @classmethod
    def from_dict(cls, d: dict[str, Any]) -> Post:
        return cls(
            uri=d["uri"],
            cid=d["cid"],
            author=Author.from_dict(d["author"]),
            text=d["text"],
            created_at=d["created_at"],
            reply_parent=d.get("reply_parent"),
            reply_root=d.get("reply_root"),
            embed_type=d.get("embed_type"),
            embed_uri=d.get("embed_uri"),
            facets=d.get("facets", []),
            labels=d.get("labels", []),
            langs=d.get("langs", []),
            like_count=d.get("like_count", 0),
            reply_count=d.get("reply_count", 0),
            repost_count=d.get("repost_count", 0),
            quote_count=d.get("quote_count", 0),
        )


@dataclass
class Thread:
    """A reply tree rooted at one post — the atomic crawl unit."""

    root_uri: str
    posts: dict[str, Post] = field(default_factory=dict)  # URI -> Post

    @property
    def post_count(self) -> int:
        return len(self.posts)

    @property
    def root_post(self) -> Post | None:
        return self.posts.get(self.root_uri)

    def to_dict(self) -> dict[str, Any]:
        return {
            "root_uri": self.root_uri,
            "posts": {uri: p.to_dict() for uri, p in self.posts.items()},
        }

    @classmethod
    def from_dict(cls, d: dict[str, Any]) -> Thread:
        thread = cls(root_uri=d["root_uri"])
        for uri, pd in d["posts"].items():
            thread.posts[uri] = Post.from_dict(pd)
        return thread


@dataclass
class QuoteEdge:
    """A quote relationship between posts, possibly across threads."""

    source: str  # URI of the quoted post
    target: str  # URI of the quoting post
    source_thread: str  # thread root URI containing the source
    target_thread: str  # thread root URI containing the target

    def to_dict(self) -> dict[str, str]:
        return {
            "source": self.source,
            "target": self.target,
            "source_thread": self.source_thread,
            "target_thread": self.target_thread,
        }

    @classmethod
    def from_dict(cls, d: dict[str, str]) -> QuoteEdge:
        return cls(
            source=d["source"],
            target=d["target"],
            source_thread=d["source_thread"],
            target_thread=d["target_thread"],
        )


@dataclass
class ContextWeb:
    """The complete crawled context graph — threads linked by quotes."""

    root_uri: str
    crawled_at: str  # ISO 8601
    threads: dict[str, Thread] = field(default_factory=dict)  # root URI -> Thread
    quote_edges: list[QuoteEdge] = field(default_factory=list)
    _post_index: dict[str, str] = field(
        default_factory=dict, init=False, repr=False,
    )  # post URI -> thread root URI, O(1) lookup

    # -- Mutation methods (keep _post_index in sync) --

    def add_thread(self, thread: Thread) -> None:
        """Register a thread and index all its posts."""
        self.threads[thread.root_uri] = thread
        for uri in thread.posts:
            self._post_index[uri] = thread.root_uri

    def remove_thread(self, root_uri: str) -> Thread:
        """Remove a thread and deindex its posts."""
        thread = self.threads.pop(root_uri)
        for uri in thread.posts:
            self._post_index.pop(uri, None)
        return thread

    def add_post(self, thread_root: str, post: Post) -> None:
        """Add a post to a thread and update the index."""
        self.threads[thread_root].posts[post.uri] = post
        self._post_index[post.uri] = thread_root

    def _rebuild_index(self) -> None:
        """Rebuild _post_index from threads (used after deserialization)."""
        self._post_index.clear()
        for thread in self.threads.values():
            for uri in thread.posts:
                self._post_index[uri] = thread.root_uri

    # -- O(1) lookup methods --

    @property
    def node_count(self) -> int:
        return len(self._post_index)

    @property
    def edge_count(self) -> int:
        reply_edges = sum(
            sum(1 for p in t.posts.values() if p.reply_parent)
            for t in self.threads.values()
        )
        return reply_edges + len(self.quote_edges)

    @property
    def thread_count(self) -> int:
        return len(self.threads)

    @property
    def nodes(self) -> dict[str, Post]:
        """Flat view of all posts across all threads (for lenses)."""
        result: dict[str, Post] = {}
        for thread in self.threads.values():
            result.update(thread.posts)
        return result

    def iter_posts(self) -> Iterator[Post]:
        """Iterate all posts without building an intermediate dict."""
        for thread in self.threads.values():
            yield from thread.posts.values()

    def has_post(self, uri: str) -> bool:
        """O(1) check if a post URI exists in any thread."""
        return uri in self._post_index

    def get_post(self, uri: str) -> Post | None:
        """O(1) lookup of a single post by URI."""
        root = self._post_index.get(uri)
        if root is None:
            return None
        return self.threads[root].posts.get(uri)

    def thread_root_for(self, uri: str) -> str | None:
        """O(1) lookup: which thread root contains this post URI?"""
        return self._post_index.get(uri)

    def thread_for_post(self, uri: str) -> Thread | None:
        """O(1) find which thread contains a given post URI."""
        root = self._post_index.get(uri)
        if root is None:
            return None
        return self.threads.get(root)

    def normalize_quote_edges(self) -> None:
        """Fix stale thread refs, drop orphans, and deduplicate quote edges."""
        seen: set[tuple[str, str]] = set()
        unique: list[QuoteEdge] = []
        for qe in self.quote_edges:
            # Drop edges referencing posts no longer in the web
            if qe.source not in self._post_index or qe.target not in self._post_index:
                continue
            # Fix stale source_thread/target_thread from placeholder merges
            qe.source_thread = self._post_index[qe.source]
            qe.target_thread = self._post_index[qe.target]
            key = (qe.source, qe.target)
            if key not in seen:
                seen.add(key)
                unique.append(qe)
        self.quote_edges = unique

    def to_dict(self) -> dict[str, Any]:
        self.normalize_quote_edges()
        return {
            "meta": {
                "format_version": 2,
                "root_uri": self.root_uri,
                "crawled_at": self.crawled_at,
                "node_count": self.node_count,
                "edge_count": self.edge_count,
                "thread_count": self.thread_count,
            },
            "threads": {uri: t.to_dict() for uri, t in self.threads.items()},
            "quote_edges": [qe.to_dict() for qe in self.quote_edges],
        }

    @classmethod
    def from_dict(cls, d: dict[str, Any]) -> ContextWeb:
        meta = d["meta"]
        web = cls(root_uri=meta["root_uri"], crawled_at=meta["crawled_at"])
        for uri, td in d["threads"].items():
            web.threads[uri] = Thread.from_dict(td)
        for qed in d.get("quote_edges", []):
            web.quote_edges.append(QuoteEdge.from_dict(qed))
        web._rebuild_index()
        return web
