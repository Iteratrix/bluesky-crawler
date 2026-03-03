"""Data models for the Context Web graph."""

from __future__ import annotations

from dataclasses import dataclass, field
from enum import Enum
from typing import Any


class EdgeType(str, Enum):
    REPLY = "reply"
    QUOTE = "quote"


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
class Edge:
    source: str  # URI
    target: str  # URI
    type: EdgeType

    def to_dict(self) -> dict[str, str]:
        return {
            "source": self.source,
            "target": self.target,
            "type": self.type.value,
        }

    @classmethod
    def from_dict(cls, d: dict[str, str]) -> Edge:
        return cls(
            source=d["source"],
            target=d["target"],
            type=EdgeType(d["type"]),
        )


@dataclass
class ContextWeb:
    """The complete crawled context graph."""

    root_uri: str
    crawled_at: str  # ISO 8601
    nodes: dict[str, Post] = field(default_factory=dict)  # URI -> Post
    edges: list[Edge] = field(default_factory=list)

    @property
    def node_count(self) -> int:
        return len(self.nodes)

    @property
    def edge_count(self) -> int:
        return len(self.edges)

    def deduplicate_edges(self) -> None:
        seen: set[tuple[str, str, str]] = set()
        unique: list[Edge] = []
        for edge in self.edges:
            key = (edge.source, edge.target, edge.type.value)
            if key not in seen:
                seen.add(key)
                unique.append(edge)
        self.edges = unique

    def to_dict(self) -> dict[str, Any]:
        self.deduplicate_edges()
        return {
            "meta": {
                "root_uri": self.root_uri,
                "crawled_at": self.crawled_at,
                "node_count": self.node_count,
                "edge_count": self.edge_count,
            },
            "nodes": {uri: post.to_dict() for uri, post in self.nodes.items()},
            "edges": [e.to_dict() for e in self.edges],
        }

    @classmethod
    def from_dict(cls, d: dict[str, Any]) -> ContextWeb:
        meta = d["meta"]
        web = cls(
            root_uri=meta["root_uri"],
            crawled_at=meta["crawled_at"],
        )
        for uri, post_data in d["nodes"].items():
            web.nodes[uri] = Post.from_dict(post_data)
        for edge_data in d["edges"]:
            web.edges.append(Edge.from_dict(edge_data))
        return web
