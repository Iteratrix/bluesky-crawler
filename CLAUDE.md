# bsky-context

Bluesky Context Web crawler — fetches the full DAG of replies + quote posts for a Bluesky conversation.

## Development

- Python project managed with `uv` (not pip/poetry)
- Run: `uv run bsky-context <command>`
- Test: `uv run pytest`
- Source: `src/bsky_context/`

## Architecture

**Thread Web**: Conversations are modeled as a collection of threads (reply trees) linked by quote edges. Each thread is the atomic crawl unit (one `getPostThread` call). Stored as JSON in `~/.local/share/bsky-context/webs/`, rendered through lenses (tree, linear, by-author, raw) on demand.

Key modules:
- `crawler.py` — thread-level BFS over getPostThread + getQuotes, with thread dedup
- `models.py` — Post, Thread, QuoteEdge, ContextWeb dataclasses
- `lenses.py` — four view renderers
- `cli.py` — Click CLI entry point
- `auth.py` — credential + session management
- `storage.py` — JSON file persistence
- `uri.py` — AT URI / bsky.app URL parsing
