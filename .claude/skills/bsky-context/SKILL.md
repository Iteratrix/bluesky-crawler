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

## Detecting the command

First, determine whether `bsky-context` is on PATH:
```bash
command -v bsky-context >/dev/null && echo "global" || echo "local"
```
- If **global**: use `bsky-context <command>`
- If **local** (running inside the project repo): use `uv run bsky-context <command>`

Use the appropriate prefix for all commands below.

## Setup

If not yet configured, run:
```bash
bsky-context auth login --handle <HANDLE> --app-password <APP_PASSWORD>
```

## Fetching a conversation

```bash
bsky-context fetch "<POST_URL>" [--max-nodes 2000] [--max-depth N] [--timeout 300]
```

- `POST_URL`: A `https://bsky.app/profile/.../post/...` URL or `at://` URI
- Prints a **web ID** to stdout (e.g. `abc123-a1b2c3`) for use with `show`
- **Automatically updates**: If a previous crawl exists for this post, it loads it and merges in new posts. Only posts with changed quote counts are re-checked for new quotes, saving API calls.
- Use `--fresh` to discard any stored version and crawl from scratch.

## Viewing a conversation

```bash
bsky-context show <WEB_ID> --lens <LENS>
```

### Lens selection guide

Choose the lens based on your reasoning task:

| Lens | Use when | What it shows |
|------|----------|---------------|
| `tree` (default) | Understanding conversation flow, who replied to whom | Indented threaded view with `[reply]`/`[quote]` tags |
| `linear` | Summarizing, understanding how discussion evolved over time | Chronological posts numbered `[1/N]` with cross-references |
| `by-author` | Analyzing each person's position, understanding a debate | Posts grouped by participant with context annotations |
| `stats` | Quick overview of a large web before diving in | Post/thread/edge counts, top authors, engagement rankings, depth distribution |
| `threads` | Finding interesting sub-conversations in a large web | Thread listing sorted by size with root post preview |
| `highlights` | Identifying key posts and people | Most quoted, most replied, highest engagement, main characters |
| `neighborhood` | Focusing on context near a specific post | Posts within N quote-hops of a target post (tree-style) |
| `timeline` | Seeing what happened in a specific time window | Time-filtered chronological view |
| `search` | Finding posts about a topic or by a specific person | Filtered results with thread context |
| `raw` | Programmatic analysis, counting, or when text views are insufficient | Full JSON graph with all metadata |

### Lens parameters

Some lenses accept additional options:

```bash
# Neighborhood: focus on N hops around a post
bsky-context show <id> -l neighborhood --hops 1
bsky-context show <id> -l neighborhood --hops 2 --uri "at://did:plc:.../post/..."

# Timeline: filter by time window
bsky-context show <id> -l timeline --after "2026-03-01T00:00:00"
bsky-context show <id> -l timeline --before "2026-03-02T00:00:00"

# Search: filter by text and/or author
bsky-context show <id> -l search -q "some topic"
bsky-context show <id> -l search --author "alice"
bsky-context show <id> -l search -q "AI" --author "simonwillison"

# Threads/highlights: control how many results
bsky-context show <id> -l threads -n 10
bsky-context show <id> -l highlights -n 5
```

## Listing cached conversations

```bash
bsky-context list
```

## Typical workflow

1. User shares a Bluesky URL
2. Fetch: `bsky-context fetch "https://bsky.app/profile/alice.bsky.social/post/xyz"`
3. Start with stats for an overview: `bsky-context show <id> -l stats`
4. For large webs, use neighborhood to focus: `bsky-context show <id> -l neighborhood --hops 1`
5. Search for specific topics or people: `bsky-context show <id> -l search -q "topic"`
6. Switch lens as needed: `tree` for flow, `by-author` for debate analysis, `highlights` for key posts
7. If the conversation is ongoing, just re-run: `bsky-context fetch "<url>"` (auto-updates)
