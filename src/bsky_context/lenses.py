"""Lens renderers for context web visualization.

Each lens transforms a ContextWeb into a string optimized for a different
reasoning task:
  - tree:      Indented threaded view (conversation flow)
  - linear:    Chronological narrative (summarization)
  - by-author: Grouped by participant (argument analysis)
  - raw:       JSON graph (programmatic use)
"""

from __future__ import annotations

import json

from bsky_context.models import ContextWeb, Edge, EdgeType, Post


def render(web: ContextWeb, lens: str = "tree") -> str:
    renderers = {
        "tree": render_tree,
        "linear": render_linear,
        "by-author": render_by_author,
        "raw": render_raw,
    }
    fn = renderers.get(lens)
    if not fn:
        raise ValueError(f"Unknown lens '{lens}'. Options: {list(renderers)}")
    return fn(web)


# ---------------------------------------------------------------------------
# Tree lens
# ---------------------------------------------------------------------------

def render_tree(web: ContextWeb) -> str:
    """Indented threaded view — DFS from root, replies and quotes nested."""
    children: dict[str, list[tuple[str, EdgeType]]] = {}
    for edge in web.edges:
        children.setdefault(edge.source, []).append((edge.target, edge.type))

    root_uri = _find_tree_root(web)
    lines: list[str] = []
    visited: set[str] = set()

    def _render(uri: str, depth: int, edge_type: EdgeType | None = None) -> None:
        if uri in visited or uri not in web.nodes:
            return
        visited.add(uri)
        post = web.nodes[uri]
        indent = "  " * depth

        tag = f"[{edge_type.value}]" if edge_type else "[root]"
        name = f"@{post.author.handle}"
        if post.author.display_name:
            name = f"{post.author.display_name} (@{post.author.handle})"

        lines.append(f"{indent}{tag} {name}  {_short_time(post.created_at)}")
        # Show text indented under the header
        for text_line in post.text.splitlines():
            lines.append(f"{indent}  {text_line}")
        if post.like_count or post.repost_count or post.quote_count:
            stats = []
            if post.like_count:
                stats.append(f"{post.like_count} likes")
            if post.repost_count:
                stats.append(f"{post.repost_count} reposts")
            if post.quote_count:
                stats.append(f"{post.quote_count} quotes")
            lines.append(f"{indent}  ({', '.join(stats)})")
        lines.append("")  # blank line between posts

        # Children: replies first (chronological), then quotes
        kids = children.get(uri, [])
        kids_sorted = sorted(kids, key=lambda x: (
            0 if x[1] == EdgeType.REPLY else 1,
            web.nodes[x[0]].created_at if x[0] in web.nodes else "",
        ))
        for child_uri, child_type in kids_sorted:
            _render(child_uri, depth + 1, child_type)

    _render(root_uri, 0)

    # Render any disconnected posts (not reachable from root)
    for uri in web.nodes:
        if uri not in visited:
            lines.append("---")
            _render(uri, 0)

    return "\n".join(lines).rstrip()


def _find_tree_root(web: ContextWeb) -> str:
    """Find the earliest ancestor in the web's node set."""
    uri = web.root_uri
    while uri in web.nodes:
        parent = web.nodes[uri].reply_parent
        if parent and parent in web.nodes:
            uri = parent
        else:
            break
    return uri


# ---------------------------------------------------------------------------
# Linear lens
# ---------------------------------------------------------------------------

def render_linear(web: ContextWeb) -> str:
    """Chronological narrative — each post numbered with context annotations."""
    posts = sorted(web.nodes.values(), key=lambda p: p.created_at)
    total = len(posts)
    uri_to_idx: dict[str, int] = {p.uri: i + 1 for i, p in enumerate(posts)}

    lines: list[str] = []
    for i, post in enumerate(posts, 1):
        name = f"@{post.author.handle}"
        if post.author.display_name:
            name = f"{post.author.display_name} (@{post.author.handle})"

        # Context annotation
        ctx_parts: list[str] = []
        if post.reply_parent and post.reply_parent in uri_to_idx:
            parent_post = web.nodes.get(post.reply_parent)
            parent_handle = f"@{parent_post.author.handle}" if parent_post else "?"
            ctx_parts.append(f"replying to {parent_handle} #{uri_to_idx[post.reply_parent]}")
        if post.embed_uri and post.embed_uri in uri_to_idx:
            quoted_post = web.nodes.get(post.embed_uri)
            quoted_handle = f"@{quoted_post.author.handle}" if quoted_post else "?"
            ctx_parts.append(f"quoting {quoted_handle} #{uri_to_idx[post.embed_uri]}")

        ctx = f"  [{', '.join(ctx_parts)}]" if ctx_parts else ""

        lines.append(f"[{i}/{total}] {name}  {_short_time(post.created_at)}{ctx}")
        for text_line in post.text.splitlines():
            lines.append(f"  {text_line}")
        lines.append("")

    return "\n".join(lines).rstrip()


# ---------------------------------------------------------------------------
# By-author lens
# ---------------------------------------------------------------------------

def render_by_author(web: ContextWeb) -> str:
    """Grouped by participant — shows each person's contributions."""
    # Group posts by author DID
    by_author: dict[str, list[Post]] = {}
    for post in web.nodes.values():
        by_author.setdefault(post.author.did, []).append(post)

    # Sort each author's posts chronologically
    for posts in by_author.values():
        posts.sort(key=lambda p: p.created_at)

    # Build URI index for cross-references
    all_posts = sorted(web.nodes.values(), key=lambda p: p.created_at)
    uri_to_idx: dict[str, int] = {p.uri: i + 1 for i, p in enumerate(all_posts)}

    # Sort authors by their first post time
    author_order = sorted(by_author.items(), key=lambda x: x[1][0].created_at)

    # Determine root author
    root_did = None
    root_uri = _find_tree_root(web)
    if root_uri in web.nodes:
        root_did = web.nodes[root_uri].author.did

    # Has quotes?
    quote_targets: set[str] = set()
    for edge in web.edges:
        if edge.type == EdgeType.QUOTE:
            quote_targets.add(edge.target)

    lines: list[str] = []
    lines.append(f"=== PARTICIPANTS ({len(author_order)}) ===")
    for did, posts in author_order:
        author = posts[0].author
        name = f"@{author.handle}"
        if author.display_name:
            name = f"{author.display_name} (@{author.handle})"
        tags: list[str] = []
        if did == root_did:
            tags.append("thread starter")
        if any(p.uri in quote_targets for p in posts):
            tags.append("via quote")
        tag_str = f"  [{', '.join(tags)}]" if tags else ""
        lines.append(f"  {name} - {len(posts)} post{'s' if len(posts) != 1 else ''}{tag_str}")
    lines.append("")

    for did, posts in author_order:
        author = posts[0].author
        name = f"@{author.handle}"
        if author.display_name:
            name = f"{author.display_name} (@{author.handle})"
        lines.append(f"=== {name} ===")

        for j, post in enumerate(posts, 1):
            ctx_parts: list[str] = []
            if post.reply_parent and post.reply_parent in web.nodes:
                parent = web.nodes[post.reply_parent]
                ctx_parts.append(f"replying to @{parent.author.handle}")
            if post.embed_uri and post.embed_uri in web.nodes:
                quoted = web.nodes[post.embed_uri]
                ctx_parts.append(f"quoting @{quoted.author.handle}")
            ctx = f"  [{', '.join(ctx_parts)}]" if ctx_parts else ""

            global_idx = uri_to_idx.get(post.uri, "?")
            lines.append(f"  [{j}] (#{global_idx}) {_short_time(post.created_at)}{ctx}")
            for text_line in post.text.splitlines():
                lines.append(f"    {text_line}")
            lines.append("")

    return "\n".join(lines).rstrip()


# ---------------------------------------------------------------------------
# Raw lens
# ---------------------------------------------------------------------------

def render_raw(web: ContextWeb) -> str:
    """JSON dump of the full graph."""
    return json.dumps(web.to_dict(), indent=2, ensure_ascii=False)


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _short_time(iso: str) -> str:
    """Shorten an ISO timestamp to a readable form."""
    # "2026-01-15T10:05:30.123Z" -> "2026-01-15 10:05"
    if not iso:
        return "?"
    return iso.replace("T", " ")[:16]
