# bsky-context

Crawl the full conversation graph of a Bluesky post — not just the linear thread, but the complete DAG of replies **and** quote posts, recursively.

Built for [Claude Code](https://docs.anthropic.com/en/docs/claude-code) but works as a standalone CLI too.

## What it does

Bluesky conversations aren't threads — they're **Context Webs**. A post gets replies (tree structure), but also gets *quoted*, and those quote posts get their own replies, and *those* get quoted... `bsky-context` crawls this entire graph and stores it locally, then renders it through different **lenses** optimized for different tasks:

| Lens | Best for | Output |
|------|----------|--------|
| `tree` | Understanding conversation flow | Indented threaded view |
| `linear` | Summarizing a discussion | Chronological narrative with cross-references |
| `by-author` | Analyzing a debate | Posts grouped by participant |
| `raw` | Programmatic use | Full JSON graph |

## Install

Requires Python 3.12+ and [uv](https://docs.astral.sh/uv/).

```bash
# Clone and install
git clone https://github.com/Iteratrix/bsky-context.git
cd bsky-context
uv sync

# Or install globally as a CLI tool
uv tool install git+https://github.com/Iteratrix/bsky-context.git
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
bsky-context show <web-id> -l raw           # JSON graph

# List cached conversations
bsky-context list
```

### Crawl controls

```bash
bsky-context fetch <url> --max-nodes 500    # cap at 500 posts
bsky-context fetch <url> --max-depth 3      # max 3 hops from start post
bsky-context fetch <url> --timeout 120      # 2 minute time limit
```

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

The crawl is a BFS over two edge types (reply and quote), with deduplication, rate-limit backoff, and configurable depth/breadth/timeout limits.

## Storage

Crawled conversations are stored as JSON files in `~/.local/share/bsky-context/webs/`. Each file contains the full graph: nodes (posts with metadata) and edges (reply/quote relationships). The format is stable and human-readable.

## Prior art

[Skythread](https://github.com/mackuba/skythread) is the closest existing tool — a web-based thread viewer that shows quote posts as a flat list under each post. It's excellent for browsing but doesn't recursively crawl into quote-post reply trees, model the result as a graph, or store anything locally. Other tools like [Skyview](https://github.com/badlogic/skyview) and [Simon Willison's thread viewer](https://tools.simonwillison.net/bluesky-thread) handle reply trees only. `bsky-context` is (as far as we know) the first tool to treat replies and quotes as a unified DAG and crawl it recursively.

## License

MIT
