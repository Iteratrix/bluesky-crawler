---
name: bsky-context
description: >
  Fetch, crawl, and analyze Bluesky conversation threads. Use when the user
  shares a bsky.app URL or AT URI and wants to understand the conversation,
  or when you need context about a Bluesky discussion. Also use when the user
  says "fetch thread", "crawl conversation", "get bsky context", or similar.
argument-hint: "[POST_URL or WEB_ID]"
---

# Bluesky Context Web Tool

This tool crawls the full **Context Web** of a Bluesky post — not just the
linear thread, but the complete DAG of replies AND quote posts, recursively.

## Setup

If not yet configured, run:
```bash
uv run bsky-context auth login --handle <HANDLE> --app-password <APP_PASSWORD>
```

## Fetching a conversation

```bash
uv run bsky-context fetch "<POST_URL>" [--max-nodes 2000] [--max-depth N] [--timeout 300]
```

- `POST_URL`: A `https://bsky.app/profile/.../post/...` URL or `at://` URI
- Prints a **web ID** to stdout (e.g. `abc123-a1b2c3`) for use with `show`

## Viewing a conversation

```bash
uv run bsky-context show <WEB_ID> --lens <LENS>
```

### Lens selection guide

Choose the lens based on your reasoning task:

| Lens | Use when | What it shows |
|------|----------|---------------|
| `tree` (default) | Understanding conversation flow, who replied to whom | Indented threaded view with `[reply]`/`[quote]` tags |
| `linear` | Summarizing, understanding how discussion evolved over time | Chronological posts numbered `[1/N]` with cross-references |
| `by-author` | Analyzing each person's position, understanding a debate | Posts grouped by participant with context annotations |
| `raw` | Programmatic analysis, counting, or when text views are insufficient | Full JSON graph with all metadata |

## Listing cached conversations

```bash
uv run bsky-context list
```

## Typical workflow

1. User shares a Bluesky URL
2. Fetch: `uv run bsky-context fetch "https://bsky.app/profile/alice.bsky.social/post/xyz"`
3. Read the tree view: `uv run bsky-context show <id>`
4. If analyzing a debate, switch lens: `uv run bsky-context show <id> -l by-author`
5. Summarize or answer questions about the conversation
