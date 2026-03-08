# Bluesky Crawler

Crawl the full conversation graph of a Bluesky post — not just the linear thread, but the complete DAG of replies **and** quote posts, recursively.

Built for [Claude Code](https://docs.anthropic.com/en/docs/claude-code) but works as a standalone CLI too.

## What it does

Bluesky conversations aren't threads — they're **Context Webs**. A post gets replies (tree structure), but also gets *quoted*, and those quote posts get their own replies, and *those* get quoted... `bsky-context` crawls this entire graph and stores it locally, then renders it through different **lenses** optimized for different tasks:

| Lens | Best for | Output |
|------|----------|--------|
| `tree` | Understanding conversation flow | Indented threaded view |
| `linear` | Summarizing a discussion | Chronological narrative with cross-references |
| `by-author` | Analyzing a debate | Posts grouped by participant |
| `stats` | Quick overview of a large web | Post/thread counts, top authors, engagement, depth distribution |
| `threads` | Finding interesting sub-conversations | Thread listing sorted by size |
| `highlights` | Identifying key posts and people | Most quoted, most replied, highest engagement |
| `neighborhood` | Focusing on nearby context | Posts within N quote-hops of a target post |
| `timeline` | Seeing how a conversation evolved | Time-windowed chronological view |
| `search` | Finding specific content or authors | Filtered results with thread context |
| `raw` | Programmatic use | Full JSON graph |

## Install

Requires Python 3.12+ and [uv](https://docs.astral.sh/uv/).

```bash
# Clone and install
git clone https://github.com/Iteratrix/bluesky-crawler.git
cd bsky-context
uv sync

# Or install globally as a CLI tool
uv tool install git+https://github.com/Iteratrix/bluesky-crawler.git
```

## Setup

Create a [Bluesky app password](https://bsky.app/settings/app-passwords), then:

```bash
bsky-context auth login
# Enter your handle and app password when prompted
```

Credentials are stored in `~/.config/bsky-context/config.json` (permissions restricted to your user).

## Usage

```bash
# Crawl a conversation
bsky-context fetch "https://bsky.app/profile/alice.bsky.social/post/abc123"

# View it
bsky-context show <web-id>                  # threaded view (default)
bsky-context show <web-id> -l linear        # chronological narrative
bsky-context show <web-id> -l by-author     # grouped by participant
bsky-context show <web-id> -l stats         # summary statistics
bsky-context show <web-id> -l threads       # thread listing by size
bsky-context show <web-id> -l highlights    # notable posts and authors
bsky-context show <web-id> -l raw           # JSON graph

# Focus on nearby context (N quote-hops)
bsky-context show <web-id> -l neighborhood --hops 1

# Filter by time window
bsky-context show <web-id> -l timeline --after "2026-03-01T00:00:00"

# Search for content or authors
bsky-context show <web-id> -l search -q "some topic"
bsky-context show <web-id> -l search --author "alice"

# List cached conversations
bsky-context list
```

### Crawl controls

```bash
bsky-context fetch <url> --max-nodes 500    # cap at 500 posts
bsky-context fetch <url> --max-depth 3      # max 3 hops from start post
bsky-context fetch <url> --timeout 120      # 2 minute time limit
bsky-context fetch <url> --fresh            # discard stored version, crawl from scratch
bsky-context fetch <url> -c 4              # use 4 concurrent API requests (default: 2)
```

Re-running `fetch` on a previously crawled post automatically loads the existing web and merges in new posts. Posts whose quote count hasn't changed are skipped for quote-fetching, making updates fast. In the rare case where a quote is deleted and a new one is created between crawls (keeping the count the same), the new quote won't be detected — use `--fresh` to force a complete re-crawl.

## Claude Code skill

This repo includes a Claude Code skill that teaches Claude when and how to use the tool. If you clone the repo and work inside it, the skill is picked up automatically.

To install the skill globally (available in any project):

```bash
cp -r .claude/skills/bsky-context ~/.claude/skills/
```

Then Claude Code can fetch and analyze Bluesky conversations mid-conversation — just share a bsky.app link and ask about it.

## How it works

1. **Fetch** the starting post's thread via `getPostThread` (reply tree + ancestors)
2. **Discover** all quote posts via `getQuotes` for every post found
3. **Recurse** — each quote post spawns its own thread crawl
4. **Store** the complete graph as JSON in `~/.local/share/bsky-context/webs/`
5. **Render** through lenses on demand

The crawl is a parallel thread-level BFS: each thread (reply tree) is the atomic unit, fetched in one API call, and quotes are the inter-thread links that drive further exploration. Multiple threads are fetched concurrently via an asyncio worker pool (default: 2 concurrent requests), with a global rate-limit pause that blocks all workers on 429 responses. Thread-level deduplication means if two quote posts point into the same thread, it's only fetched once. Configurable depth/breadth/timeout/concurrency limits keep things under control.

## Storage

Crawled conversations are stored as JSON files in `~/.local/share/bsky-context/webs/`. Each file contains the full graph: threads (reply trees keyed by root URI) and quote edges (cross-thread links). The format is stable and human-readable.

## Prior art

[Skythread](https://github.com/mackuba/skythread) is the closest existing tool — a web-based thread viewer that shows quote posts as a flat list under each post. It's excellent for browsing but doesn't recursively crawl into quote-post reply trees, model the result as a graph, or store anything locally. Other tools like [Skyview](https://github.com/badlogic/skyview) and [Simon Willison's thread viewer](https://tools.simonwillison.net/bluesky-thread) handle reply trees only. `bsky-context` is (as far as we know) the first tool to treat replies and quotes as a unified DAG and crawl it recursively.

## License

MIT
