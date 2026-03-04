"""Lens renderers for context web visualization.

Each lens transforms a ContextWeb into a string optimized for a different
reasoning task:
  - tree:         Indented threaded view (conversation flow)
  - linear:       Chronological narrative (summarization)
  - by-author:    Grouped by participant (argument analysis)
  - raw:          JSON graph (programmatic use)
  - stats:        Summary statistics (quick overview)
  - threads:      Thread listing sorted by size/engagement
  - highlights:   Most notable posts and authors
  - neighborhood: N-hop subgraph around a post
  - timeline:     Time-windowed chronological view
  - search:       Filter by text content or author
"""

from __future__ import annotations

import json
from collections import deque

from bsky_context.models import ContextWeb, Post


def render(web: ContextWeb, lens: str = "tree", **kwargs) -> str:
    renderers = {
        "tree": render_tree,
        "linear": render_linear,
        "by-author": render_by_author,
        "raw": render_raw,
        "stats": render_stats,
        "threads": render_threads,
        "highlights": render_highlights,
        "neighborhood": render_neighborhood,
        "timeline": render_timeline,
        "search": render_search,
    }
    fn = renderers.get(lens)
    if not fn:
        raise ValueError(f"Unknown lens '{lens}'. Options: {list(renderers)}")
    return fn(web, **kwargs)


# ---------------------------------------------------------------------------
# Shared helpers
# ---------------------------------------------------------------------------

def _short_time(iso: str) -> str:
    """Shorten an ISO timestamp to a readable form."""
    # "2026-01-15T10:05:30.123Z" -> "2026-01-15 10:05"
    if not iso:
        return "?"
    return iso.replace("T", " ")[:16]


def _author_name(post: Post) -> str:
    if post.author.display_name:
        return f"{post.author.display_name} (@{post.author.handle})"
    return f"@{post.author.handle}"


def _truncate(text: str, max_len: int = 80) -> str:
    text = text.replace("\n", " ").strip()
    if len(text) <= max_len:
        return text
    return text[:max_len - 3] + "..."


def _engagement(post: Post) -> int:
    return post.like_count + post.repost_count + post.quote_count


def _build_children(web: ContextWeb) -> dict[str, list[str]]:
    """Map parent URI -> list of reply URIs."""
    children: dict[str, list[str]] = {}
    for thread in web.threads.values():
        for post in thread.posts.values():
            if post.reply_parent:
                children.setdefault(post.reply_parent, []).append(post.uri)
    return children


def _build_quotes_received(web: ContextWeb) -> dict[str, int]:
    """Map source URI -> count of posts that quote it."""
    counts: dict[str, int] = {}
    for qe in web.quote_edges:
        counts[qe.source] = counts.get(qe.source, 0) + 1
    return counts


def _thread_hop_distances(web: ContextWeb, start_thread: str) -> dict[str, int]:
    """BFS over quote edges to find hop distance of each thread from start_thread."""
    adj: dict[str, set[str]] = {}
    for qe in web.quote_edges:
        adj.setdefault(qe.source_thread, set()).add(qe.target_thread)
        adj.setdefault(qe.target_thread, set()).add(qe.source_thread)

    distances: dict[str, int] = {start_thread: 0}
    queue = deque([start_thread])
    while queue:
        t = queue.popleft()
        for neighbor in adj.get(t, set()):
            if neighbor not in distances:
                distances[neighbor] = distances[t] + 1
                queue.append(neighbor)
    return distances


def _find_tree_root(web: ContextWeb) -> str:
    """Find the earliest ancestor in the web's node set."""
    nodes = web.nodes
    uri = web.root_uri
    while uri in nodes:
        parent = nodes[uri].reply_parent
        if parent and parent in nodes:
            uri = parent
        else:
            break
    return uri


# ---------------------------------------------------------------------------
# Tree lens
# ---------------------------------------------------------------------------

def render_tree(web: ContextWeb, **kwargs) -> str:
    """Indented threaded view — DFS from root, replies and quotes nested."""
    nodes = web.nodes
    children: dict[str, list[tuple[str, str]]] = {}  # uri -> [(child_uri, "reply"|"quote")]

    # Reply edges: from reply_parent within threads
    for thread in web.threads.values():
        for post in thread.posts.values():
            if post.reply_parent:
                children.setdefault(post.reply_parent, []).append((post.uri, "reply"))

    # Quote edges
    for qe in web.quote_edges:
        children.setdefault(qe.source, []).append((qe.target, "quote"))

    root_uri = _find_tree_root(web)
    lines: list[str] = []
    visited: set[str] = set()

    def _render(uri: str, depth: int, edge_type: str | None = None) -> None:
        if uri in visited or uri not in nodes:
            return
        visited.add(uri)
        post = nodes[uri]
        indent = "  " * depth

        tag = f"[{edge_type}]" if edge_type else "[root]"
        name = _author_name(post)

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
            0 if x[1] == "reply" else 1,
            nodes[x[0]].created_at if x[0] in nodes else "",
        ))
        for child_uri, child_type in kids_sorted:
            _render(child_uri, depth + 1, child_type)

    _render(root_uri, 0)

    # Render any disconnected posts (not reachable from root)
    for uri in nodes:
        if uri not in visited:
            lines.append("---")
            _render(uri, 0)

    return "\n".join(lines).rstrip()


# ---------------------------------------------------------------------------
# Linear lens
# ---------------------------------------------------------------------------

def render_linear(web: ContextWeb, **kwargs) -> str:
    """Chronological narrative — each post numbered with context annotations."""
    nodes = web.nodes
    posts = sorted(nodes.values(), key=lambda p: p.created_at)
    total = len(posts)
    uri_to_idx: dict[str, int] = {p.uri: i + 1 for i, p in enumerate(posts)}

    lines: list[str] = []
    for i, post in enumerate(posts, 1):
        name = _author_name(post)

        # Context annotation
        ctx_parts: list[str] = []
        if post.reply_parent and post.reply_parent in uri_to_idx:
            parent_post = nodes.get(post.reply_parent)
            parent_handle = f"@{parent_post.author.handle}" if parent_post else "?"
            ctx_parts.append(f"replying to {parent_handle} #{uri_to_idx[post.reply_parent]}")
        if post.embed_uri and post.embed_uri in uri_to_idx:
            quoted_post = nodes.get(post.embed_uri)
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

def render_by_author(web: ContextWeb, **kwargs) -> str:
    """Grouped by participant — shows each person's contributions."""
    nodes = web.nodes

    # Group posts by author DID
    by_author: dict[str, list] = {}
    for post in nodes.values():
        by_author.setdefault(post.author.did, []).append(post)

    # Sort each author's posts chronologically
    for posts in by_author.values():
        posts.sort(key=lambda p: p.created_at)

    # Build URI index for cross-references
    all_posts = sorted(nodes.values(), key=lambda p: p.created_at)
    uri_to_idx: dict[str, int] = {p.uri: i + 1 for i, p in enumerate(all_posts)}

    # Sort authors by their first post time
    author_order = sorted(by_author.items(), key=lambda x: x[1][0].created_at)

    # Determine root author
    root_did = None
    root_uri = _find_tree_root(web)
    if root_uri in nodes:
        root_did = nodes[root_uri].author.did

    # Has quotes?
    quote_targets: set[str] = {qe.target for qe in web.quote_edges}

    lines: list[str] = []
    lines.append(f"=== PARTICIPANTS ({len(author_order)}) ===")
    for did, posts in author_order:
        author = posts[0].author
        name = _author_name(posts[0])
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
        name = _author_name(posts[0])
        lines.append(f"=== {name} ===")

        for j, post in enumerate(posts, 1):
            ctx_parts: list[str] = []
            if post.reply_parent and post.reply_parent in nodes:
                parent = nodes[post.reply_parent]
                ctx_parts.append(f"replying to @{parent.author.handle}")
            if post.embed_uri and post.embed_uri in nodes:
                quoted = nodes[post.embed_uri]
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

def render_raw(web: ContextWeb, **kwargs) -> str:
    """JSON dump of the full graph."""
    return json.dumps(web.to_dict(), indent=2, ensure_ascii=False)


# ---------------------------------------------------------------------------
# Stats lens
# ---------------------------------------------------------------------------

def render_stats(web: ContextWeb, **kwargs) -> str:
    """Summary statistics — quick overview of a context web."""
    lines: list[str] = []
    lines.append("=== CONTEXT WEB STATISTICS ===")
    lines.append("")

    # Counts
    reply_edges = sum(
        1 for p in web.iter_posts() if p.reply_parent
    )
    lines.append(f"Posts: {web.node_count:,} across {web.thread_count:,} threads")
    lines.append(
        f"Edges: {reply_edges:,} reply + {len(web.quote_edges):,} quote "
        f"= {reply_edges + len(web.quote_edges):,} total"
    )

    # Time span
    times = [p.created_at for p in web.iter_posts() if p.created_at]
    if times:
        lines.append(f"Time span: {_short_time(min(times))} to {_short_time(max(times))}")
    lines.append("")

    # Thread size distribution
    sizes = [len(t.posts) for t in web.threads.values()]
    buckets = [
        ("1 post", sum(1 for s in sizes if s == 1)),
        ("2-10 posts", sum(1 for s in sizes if 2 <= s <= 10)),
        ("11-100 posts", sum(1 for s in sizes if 11 <= s <= 100)),
        ("100+ posts", sum(1 for s in sizes if s > 100)),
    ]
    lines.append("Thread sizes:")
    for label, count in buckets:
        pct = count / len(sizes) * 100 if sizes else 0
        lines.append(f"  {label:15s} {count:,} threads ({pct:.1f}%)")
    if sizes:
        largest = max(sizes)
        lines.append(f"  Largest: {largest:,} posts")
    lines.append("")

    # Top authors by post count
    author_counts: dict[str, tuple[str, int]] = {}  # handle -> (display, count)
    for post in web.iter_posts():
        handle = post.author.handle
        if handle not in author_counts:
            author_counts[handle] = (_author_name(post), 0)
        author_counts[handle] = (author_counts[handle][0], author_counts[handle][1] + 1)
    top_authors = sorted(author_counts.items(), key=lambda x: x[1][1], reverse=True)[:10]
    lines.append("Top authors by post count:")
    for i, (handle, (name, count)) in enumerate(top_authors, 1):
        lines.append(f"  {i:2d}. {name} - {count:,} posts")
    lines.append("")

    # Top posts by engagement
    top_posts = sorted(web.iter_posts(), key=_engagement, reverse=True)[:10]
    lines.append("Top posts by engagement:")
    for i, post in enumerate(top_posts, 1):
        eng = _engagement(post)
        lines.append(f"  {i:2d}. [{eng:,} engagement] {_author_name(post)}")
        lines.append(f"      {_truncate(post.text)}")
    lines.append("")

    # Quote-hop depth distribution
    root_thread = web.thread_root_for(web.root_uri)
    if root_thread:
        distances = _thread_hop_distances(web, root_thread)
        depth_posts: dict[int, int] = {}
        for thread_root, dist in distances.items():
            thread = web.threads.get(thread_root)
            if thread:
                depth_posts[dist] = depth_posts.get(dist, 0) + len(thread.posts)
        # Posts in threads not reachable from root
        unreachable = web.node_count - sum(depth_posts.values())

        lines.append("Quote-hop depth from root thread:")
        for d in sorted(depth_posts):
            lines.append(f"  Hop {d}: {depth_posts[d]:,} posts")
        if unreachable > 0:
            lines.append(f"  Unreachable: {unreachable:,} posts")

    return "\n".join(lines).rstrip()


# ---------------------------------------------------------------------------
# Threads lens
# ---------------------------------------------------------------------------

def render_threads(web: ContextWeb, *, top: int = 20, **kwargs) -> str:
    """List threads sorted by size."""
    thread_info: list[tuple[int, int, str, str, str]] = []  # (size, engagement, name, text, uri)
    for thread in web.threads.values():
        size = len(thread.posts)
        eng = sum(_engagement(p) for p in thread.posts.values())
        root_post = thread.root_post
        if root_post:
            name = _author_name(root_post)
            text = _truncate(root_post.text)
            uri = root_post.uri
        else:
            # Root post might not be in thread (placeholder); use first post
            first = next(iter(thread.posts.values()), None)
            if first:
                name = _author_name(first)
                text = _truncate(first.text)
                uri = first.uri
            else:
                continue
        thread_info.append((size, eng, name, text, uri))

    thread_info.sort(key=lambda x: x[0], reverse=True)

    lines: list[str] = []
    lines.append(f"=== THREADS ({web.thread_count:,} total, showing top {min(top, len(thread_info))}) ===")
    lines.append("")
    for i, (size, eng, name, text, uri) in enumerate(thread_info[:top], 1):
        lines.append(f"#{i:<3d} {size:,} posts | {eng:,} engagement | {name}")
        lines.append(f"     {text}")
        lines.append(f"     {uri}")
        lines.append("")

    return "\n".join(lines).rstrip()


# ---------------------------------------------------------------------------
# Highlights lens
# ---------------------------------------------------------------------------

def render_highlights(web: ContextWeb, *, top: int = 10, **kwargs) -> str:
    """Surface the most notable posts and authors."""
    nodes = web.nodes
    lines: list[str] = []
    lines.append("=== HIGHLIGHTS ===")
    lines.append("")

    # Most quoted
    quotes_received = _build_quotes_received(web)
    if quotes_received:
        top_quoted = sorted(quotes_received.items(), key=lambda x: x[1], reverse=True)[:top]
        lines.append("--- Most Quoted ---")
        for i, (uri, count) in enumerate(top_quoted, 1):
            post = web.get_post(uri)
            if not post:
                continue
            lines.append(f"  {i}. [quoted {count} times] {_author_name(post)}  {_short_time(post.created_at)}")
            lines.append(f"     {_truncate(post.text)}")
            lines.append(f"     ({post.like_count:,} likes, {post.repost_count:,} reposts)")
            lines.append("")

    # Most replied (in-web reply count)
    children = _build_children(web)
    if children:
        reply_counts = {uri: len(kids) for uri, kids in children.items()}
        top_replied = sorted(reply_counts.items(), key=lambda x: x[1], reverse=True)[:top]
        lines.append("--- Most Replied ---")
        for i, (uri, count) in enumerate(top_replied, 1):
            post = web.get_post(uri)
            if not post:
                continue
            lines.append(f"  {i}. [{count} replies in web] {_author_name(post)}  {_short_time(post.created_at)}")
            lines.append(f"     {_truncate(post.text)}")
            lines.append("")

    # Highest engagement
    top_eng = sorted(web.iter_posts(), key=_engagement, reverse=True)[:top]
    lines.append("--- Highest Engagement ---")
    for i, post in enumerate(top_eng, 1):
        lines.append(
            f"  {i}. [{post.like_count:,} likes, {post.repost_count:,} reposts, "
            f"{post.quote_count:,} quotes] {_author_name(post)}"
        )
        lines.append(f"     {_truncate(post.text)}")
        lines.append("")

    # Main characters — authors by total engagement received
    author_eng: dict[str, tuple[str, int]] = {}  # handle -> (display_name, total_engagement)
    for post in web.iter_posts():
        handle = post.author.handle
        eng = _engagement(post)
        if handle not in author_eng:
            author_eng[handle] = (_author_name(post), 0)
        author_eng[handle] = (author_eng[handle][0], author_eng[handle][1] + eng)
    top_characters = sorted(author_eng.items(), key=lambda x: x[1][1], reverse=True)[:top]
    lines.append("--- Main Characters (by total engagement) ---")
    for i, (handle, (name, eng)) in enumerate(top_characters, 1):
        lines.append(f"  {i}. {name} - {eng:,} total engagement")
    lines.append("")

    return "\n".join(lines).rstrip()


# ---------------------------------------------------------------------------
# Neighborhood lens
# ---------------------------------------------------------------------------

def render_neighborhood(web: ContextWeb, *, uri: str | None = None, hops: int = 2, **kwargs) -> str:
    """Render posts within N quote-hops of a target post."""
    target_uri = uri or web.root_uri
    target_thread = web.thread_root_for(target_uri)
    if not target_thread:
        return f"Post not found in web: {target_uri}"

    distances = _thread_hop_distances(web, target_thread)
    included_threads = {t for t, d in distances.items() if d <= hops}

    # Build filtered node set
    nodes: dict[str, Post] = {}
    for thread_root in included_threads:
        thread = web.threads.get(thread_root)
        if thread:
            nodes.update(thread.posts)

    # Build children from included posts only
    children: dict[str, list[tuple[str, str]]] = {}
    for post in nodes.values():
        if post.reply_parent and post.reply_parent in nodes:
            children.setdefault(post.reply_parent, []).append((post.uri, "reply"))
    for qe in web.quote_edges:
        if qe.source in nodes and qe.target in nodes:
            children.setdefault(qe.source, []).append((qe.target, "quote"))

    lines: list[str] = []
    lines.append(f"=== NEIGHBORHOOD ({hops} hops from target) ===")
    lines.append(f"Posts: {len(nodes):,} of {web.node_count:,} | Threads: {len(included_threads):,} of {web.thread_count:,}")
    lines.append("")

    # DFS render (same logic as tree lens but with filtered nodes)
    root_uri = _find_tree_root(web)
    if root_uri not in nodes:
        # Fall back to target URI if tree root isn't in neighborhood
        root_uri = target_uri

    visited: set[str] = set()

    def _render(post_uri: str, depth: int, edge_type: str | None = None) -> None:
        if post_uri in visited or post_uri not in nodes:
            return
        visited.add(post_uri)
        post = nodes[post_uri]
        indent = "  " * depth

        tag = f"[{edge_type}]" if edge_type else "[root]"
        name = _author_name(post)

        lines.append(f"{indent}{tag} {name}  {_short_time(post.created_at)}")
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
        lines.append("")

        kids = children.get(post_uri, [])
        kids_sorted = sorted(kids, key=lambda x: (
            0 if x[1] == "reply" else 1,
            nodes[x[0]].created_at if x[0] in nodes else "",
        ))
        for child_uri, child_type in kids_sorted:
            _render(child_uri, depth + 1, child_type)

    _render(root_uri, 0)

    for post_uri in nodes:
        if post_uri not in visited:
            lines.append("---")
            _render(post_uri, 0)

    return "\n".join(lines).rstrip()


# ---------------------------------------------------------------------------
# Timeline lens
# ---------------------------------------------------------------------------

def render_timeline(web: ContextWeb, *, after: str | None = None, before: str | None = None, **kwargs) -> str:
    """Time-windowed chronological view."""
    nodes = web.nodes
    posts = sorted(nodes.values(), key=lambda p: p.created_at)

    # Filter by time window
    if after:
        posts = [p for p in posts if p.created_at >= after]
    if before:
        posts = [p for p in posts if p.created_at < before]

    total = len(posts)
    uri_to_idx: dict[str, int] = {p.uri: i + 1 for i, p in enumerate(posts)}

    lines: list[str] = []
    window_desc = ""
    if after and before:
        window_desc = f"{_short_time(after)} to {_short_time(before)}"
    elif after:
        window_desc = f"after {_short_time(after)}"
    elif before:
        window_desc = f"before {_short_time(before)}"
    else:
        window_desc = "all time"

    lines.append(f"=== TIMELINE ({window_desc}) ===")
    lines.append(f"Posts: {total:,} of {web.node_count:,}")
    lines.append("")

    for i, post in enumerate(posts, 1):
        name = _author_name(post)

        ctx_parts: list[str] = []
        if post.reply_parent:
            parent_post = nodes.get(post.reply_parent)
            if parent_post:
                parent_handle = f"@{parent_post.author.handle}"
                if post.reply_parent in uri_to_idx:
                    ctx_parts.append(f"replying to {parent_handle} #{uri_to_idx[post.reply_parent]}")
                else:
                    ctx_parts.append(f"replying to {parent_handle}")
        if post.embed_uri:
            quoted_post = nodes.get(post.embed_uri)
            if quoted_post:
                quoted_handle = f"@{quoted_post.author.handle}"
                if post.embed_uri in uri_to_idx:
                    ctx_parts.append(f"quoting {quoted_handle} #{uri_to_idx[post.embed_uri]}")
                else:
                    ctx_parts.append(f"quoting {quoted_handle}")

        ctx = f"  [{', '.join(ctx_parts)}]" if ctx_parts else ""

        lines.append(f"[{i}/{total}] {name}  {_short_time(post.created_at)}{ctx}")
        for text_line in post.text.splitlines():
            lines.append(f"  {text_line}")
        lines.append("")

    return "\n".join(lines).rstrip()


# ---------------------------------------------------------------------------
# Search lens
# ---------------------------------------------------------------------------

def render_search(web: ContextWeb, *, query: str | None = None, author: str | None = None, **kwargs) -> str:
    """Filter posts by text content and/or author handle."""
    if not query and not author:
        return "No search criteria provided. Use --query and/or --author."

    nodes = web.nodes
    matches: list[Post] = []

    query_lower = query.lower() if query else None
    author_lower = author.lower() if author else None

    for post in sorted(web.iter_posts(), key=lambda p: p.created_at):
        if query_lower and query_lower not in post.text.lower():
            continue
        if author_lower and author_lower not in post.author.handle.lower():
            continue
        matches.append(post)

    lines: list[str] = []
    filter_desc = []
    if query:
        filter_desc.append(f'query: "{query}"')
    if author:
        filter_desc.append(f"author: {author}")
    lines.append(f"=== SEARCH RESULTS ===")
    lines.append(f"{' | '.join(filter_desc)} | {len(matches):,} matches in {web.node_count:,} posts")
    lines.append("")

    for i, post in enumerate(matches, 1):
        name = _author_name(post)
        thread_root = web.thread_root_for(post.uri)
        thread = web.threads.get(thread_root) if thread_root else None
        thread_size = len(thread.posts) if thread else 0

        lines.append(f"[{i}] {name}  {_short_time(post.created_at)}")
        lines.append(f"    Thread: {thread_root} ({thread_size:,} posts)")

        ctx_parts: list[str] = []
        if post.reply_parent:
            parent_post = nodes.get(post.reply_parent)
            if parent_post:
                ctx_parts.append(f"replying to @{parent_post.author.handle}")
        if post.embed_uri:
            quoted_post = nodes.get(post.embed_uri)
            if quoted_post:
                ctx_parts.append(f"quoting @{quoted_post.author.handle}")
        if ctx_parts:
            lines.append(f"    [{', '.join(ctx_parts)}]")

        lines.append(f"    {_truncate(post.text, 120)}")
        if _engagement(post) > 0:
            lines.append(f"    ({post.like_count:,} likes, {post.repost_count:,} reposts, {post.quote_count:,} quotes)")
        lines.append("")

    return "\n".join(lines).rstrip()
