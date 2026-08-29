# bsky-context

Crawl the full conversation graph of a Bluesky post — not just the linear thread, but the complete DAG of replies **and** quote posts, recursively — and render it as text a person or a language model can reason over.

Three ways to use it, all running the same Rust core:

| | For | Where it runs |
|---|---|---|
| **CLI** `bsky-context` | You at a terminal, or Claude Code via the bundled skill | Natively, stores webs locally |
| **Web app** | Anyone in a browser | Entirely client-side (WASM) on GitHub Pages; nothing leaves your machine |
| **MCP server** | Claude (or any MCP client) as a tool it can call itself | A Cloudflare Worker: one `bsky_context` tool, no auth, cached crawls |

No login needed: Bluesky's public AppView serves everything the crawler uses unauthenticated.

## Quick start

```bash
# Install the CLI
cargo install --git https://github.com/Iteratrix/bluesky-crawler bsky-context-cli

# Crawl a conversation and look at it
bsky-context fetch "https://bsky.app/profile/alice.bsky.social/post/abc123"
bsky-context show <web-id>            # threaded view
bsky-context show <web-id> -l stats   # overview first for big webs
```

**As a Claude Code skill:** copy `.claude/skills/bsky-context` to `~/.claude/skills/` and Claude can fetch and analyze any bsky.app link you share mid-conversation.

**From Claude, ChatGPT, Cursor, or any MCP client:** connect the MCP server; see [Connect the MCP server](#connect-the-mcp-server). The client gets a `bsky_context` tool it calls whenever a Bluesky post comes up, choosing lenses itself.

## Connect the MCP server

A public instance runs at `https://bsky-context.mimirs.workers.dev/mcp` (Streamable HTTP, no authentication, best effort; self-host with the instructions below if you depend on it). The server exposes one tool, `bsky_context`, and tells the model when to use it.

| Client | How |
|---|---|
| Claude (web, desktop, mobile; Pro/Max/Team) | Settings → Connectors → Add custom connector → URL `https://bsky-context.mimirs.workers.dev/mcp`, no OAuth |
| Claude Code | `claude mcp add --transport http bsky-context https://bsky-context.mimirs.workers.dev/mcp` |
| Cursor | `.cursor/mcp.json`: `{"mcpServers":{"bsky-context":{"url":"https://bsky-context.mimirs.workers.dev/mcp"}}}` |
| ChatGPT | Settings → Connectors → Advanced → Developer mode → Create, with the same URL (custom connectors require Plus/Pro/Team/Enterprise) |
| Anything else | Any client that accepts a remote MCP server URL over Streamable HTTP |

Then ask about a Bluesky link. The tool's description carries the lens guide, so no prompt engineering is needed; `lens=stats` first, then `highlights`, `neighborhood`, or `search` is the workflow the model is nudged toward on large conversations.

## What it does

Bluesky conversations aren't threads — they're **Context Webs**. A post gets replies (tree structure), but also gets *quoted*, and those quote posts get their own replies, and *those* get quoted... `bsky-context` crawls this entire graph, stores it, and renders it through **lenses** optimized for different tasks:

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

## CLI

```bash
bsky-context fetch <url-or-at-uri> [--max-nodes 2000] [--max-depth N] [--timeout 300] [-c 2] [--fresh] [-v]
bsky-context show <web-id> [-l LENS] [--hops N] [--uri U] [--after T] [--before T] [-q Q] [--author A] [-n TOP]
bsky-context list
```

`fetch` prints a web ID; `show` accepts that ID or any unique prefix. Re-running `fetch` on a known post loads the stored web and merges in what's new: posts whose quote count hasn't changed are skipped for quote-fetching, so updates are fast. `--fresh` discards the stored version (use it if a quote may have been deleted and recreated, which keeps the count the same). `-c` sets concurrent API requests; higher is faster but risks rate limits.

Webs are stored as JSON in `~/.local/share/bsky-context/webs/` (honors `XDG_DATA_HOME`). The format is stable and human-readable, and unchanged from the original Python implementation, so webs it saved load as-is.

## Web app

Open the deployed page, paste a post URL, crawl. The crawl runs in your browser against `public.api.bsky.app`; lenses switch instantly once the web is loaded, and **Save JSON** downloads the same file the CLI would store. The page works offline after the first load (service worker).

Dev loop:

```bash
wasm-pack build bsky-context-web --target web --out-dir ../web/pkg
python3 -m http.server -d web        # any static server; the service worker is skipped on localhost
```

Deploy: push a version tag (`git tag v0.1.0 && git push origin v0.1.0`). One-time setup: repo Settings → Pages → Source: "GitHub Actions".

## MCP server (Cloudflare Worker)

A remote MCP server (Streamable HTTP, no authentication) exposing one tool:

```
bsky_context(post, lens?, top?, hops?, uri?, after?, before?, query?, author?, fresh?)
```

`post` is a bsky.app URL or `at://` URI; the other arguments are the lens parameters from the table above. The result is text: a short header (counts, whether the crawl finished) followed by the lens output.

Crawls are bounded per call (`CRAWL_MAX_NODES`, `CRAWL_TIMEOUT_SECS`, `CRAWL_CONCURRENCY` in `wrangler.toml`; defaults 500 posts / 20 s / 4) because MCP clients time out tool calls. When the budget is hit the result says so and how many threads are unexplored; calling again with the same post continues from the cached web. With a `WEBS` KV namespace bound (`wrangler kv namespace create WEBS`, paste the id into `wrangler.toml`), a call within `CACHE_FRESH_SECS` (default 300) of the last crawl renders the cached web without crawling, so switching lenses is instant; entries expire 30 days after their last update. Without KV every call crawls from scratch.

```bash
cargo install worker-build            # needs OpenSSL headers (libssl-dev)
cd bsky-context-worker
npx wrangler dev                      # local; POST JSON-RPC to http://localhost:8787/mcp
npx wrangler deploy                   # or run the "Deploy Cloudflare Worker" workflow
npx wrangler tail                     # live logs: crawl warnings, KV failures
```

## How it works

1. **Fetch** the starting post's thread via `getPostThread` (reply tree + ancestors)
2. **Discover** all quote posts via `getQuotes` for every post found
3. **Recurse** — each quote post spawns its own thread crawl
4. **Store** the complete graph as JSON
5. **Render** through lenses on demand

The crawl is a thread-level BFS: each thread (reply tree) is the atomic unit, fetched in one API call, and quotes are the inter-thread links that drive further exploration. Requests run concurrently up to `-c`, with a global pause on 429 responses. Thread-level deduplication means two quote posts pointing into the same thread fetch it once. Depth, breadth, timeout, and concurrency limits keep it under control.

## Layout

Pure core, thin adapters. `bsky-context-core` holds the data model, crawler, and lenses and does no I/O; HTTP and time come in through two small traits, so the identical crawler runs natively, in a browser, and in a Worker.

```
bsky-context-core/     model, uri, api (wire types + Fetch/Clock traits), crawler, lens/
bsky-context-cli/      the bsky-context binary
bsky-context-web/      wasm-bindgen bridge      web/   framework-free page, build.mjs, service worker
bsky-context-worker/   Cloudflare Worker (MCP)  .claude/skills/bsky-context/   Claude Code skill
```

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

## Prior art

[Skythread](https://github.com/mackuba/skythread) is the closest existing tool — a web-based thread viewer that shows quote posts as a flat list under each post. It's excellent for browsing but doesn't recursively crawl into quote-post reply trees, model the result as a graph, or store anything locally. Other tools like [Skyview](https://github.com/badlogic/skyview) and [Simon Willison's thread viewer](https://tools.simonwillison.net/bluesky-thread) handle reply trees only. `bsky-context` is (as far as we know) the first tool to treat replies and quotes as a unified DAG and crawl it recursively.

## License

MIT
