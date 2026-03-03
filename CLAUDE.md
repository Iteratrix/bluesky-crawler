# bsky-context

Bluesky Context Web crawler — fetches the full DAG of replies + quote posts for a Bluesky conversation.

## Development

- Python project managed with `uv` (not pip/poetry)
- Run: `uv run bsky-context <command>`
- Test: `uv run pytest`
- Source: `src/bsky_context/`

## Architecture

**Storage/Lens Split**: Canonical JSON graph stored in `~/.local/share/bsky-context/webs/`, rendered through lenses (tree, linear, by-author, raw) on demand.

Key modules:
- `crawler.py` — async BFS over getPostThread + getQuotes
- `models.py` — Post, Edge, ContextWeb dataclasses
- `lenses.py` — four view renderers
- `cli.py` — Click CLI entry point
- `auth.py` — credential + session management
- `storage.py` — JSON file persistence
- `uri.py` — AT URI / bsky.app URL parsing
