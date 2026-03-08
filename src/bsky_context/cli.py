"""CLI entry point for bsky-context."""

from __future__ import annotations

import asyncio
import logging
import sys
import time

import click

from bsky_context.lenses import render
from bsky_context.storage import list_webs, load_web, save_web
from bsky_context.uri import PostRef


@click.group()
def main():
    """Crawl and explore Bluesky conversation graphs."""


@main.command()
@click.argument("post_url")
@click.option("--max-nodes", default=2000, show_default=True,
              help="Maximum posts to crawl.")
@click.option("--max-depth", default=None, type=int,
              help="Maximum BFS hop distance from start post.")
@click.option("--timeout", default=300.0, show_default=True,
              help="Maximum wall-clock seconds for the crawl.")
@click.option("--fresh", is_flag=True, default=False,
              help="Discard stored version and crawl from scratch. "
                   "Use when quotes may have been deleted and recreated "
                   "(smart re-fetch can miss these since it checks counts, not URIs).")
@click.option("--verbose", "-v", is_flag=True, default=False,
              help="Show detailed logging (rate limits, retries, errors).")
def fetch(post_url: str, max_nodes: int, max_depth: int | None, timeout: float,
          fresh: bool, verbose: bool):
    """Crawl a Bluesky conversation graph starting from POST_URL.

    POST_URL can be an AT URI or a bsky.app URL.

    If a previous crawl exists for this post, it is automatically loaded and
    updated with new posts. Use --fresh to discard the stored version and
    start over.
    """
    try:
        ref = PostRef.from_str(post_url)
    except ValueError as e:
        click.echo(f"Error: {e}", err=True)
        sys.exit(1)

    if verbose:
        logging.basicConfig(
            level=logging.DEBUG,
            format="  %(levelname)s %(name)s: %(message)s",
            stream=sys.stderr,
        )
    else:
        logging.basicConfig(
            level=logging.WARNING,
            format="  %(levelname)s: %(message)s",
            stream=sys.stderr,
        )

    async def _run():
        from bsky_context.auth import get_client
        from bsky_context.crawler import crawl

        try:
            client = await get_client()
        except RuntimeError as e:
            click.echo(f"Error: {e}", err=True)
            sys.exit(1)

        # Auto-load existing web unless --fresh
        existing = None
        if not fresh:
            try:
                existing = load_web(ref.rkey)
                click.echo(
                    f"  Updating existing web ({existing.node_count} posts)...",
                    err=True,
                )
            except FileNotFoundError:
                pass  # No existing web, fresh crawl

        t0 = time.monotonic()

        def _progress(nodes: int, edges: int, threads: int) -> None:
            elapsed = time.monotonic() - t0
            click.echo(
                f"\r  Crawling... {nodes} posts, {threads} threads, {edges} edges ({elapsed:.0f}s)",
                nl=False, err=True,
            )

        web = await crawl(
            client,
            ref.at_uri,
            max_nodes=max_nodes,
            max_depth=max_depth,
            timeout=timeout,
            existing=existing,
            progress_callback=_progress,
        )

        elapsed = time.monotonic() - t0
        path = save_web(web)
        click.echo("", err=True)  # newline after progress
        click.echo(
            f"  Done in {elapsed:.1f}s: {web.node_count} posts, {web.thread_count} threads, {web.edge_count} edges",
            err=True,
        )
        click.echo(f"  Saved: {path.stem}", err=True)
        # Machine-consumable output: just the web ID
        click.echo(path.stem)

    asyncio.run(_run())


@main.command()
@click.argument("web_id")
@click.option("--lens", "-l", default="tree",
              type=click.Choice(["tree", "linear", "by-author", "raw",
                                 "stats", "threads", "highlights",
                                 "neighborhood", "timeline", "search"]),
              help="View to render.")
@click.option("--hops", default=None, type=int,
              help="Quote-chain hops (neighborhood lens).")
@click.option("--uri", default=None,
              help="Target post URI (neighborhood lens).")
@click.option("--after", default=None,
              help="Show posts after this ISO timestamp (timeline lens).")
@click.option("--before", default=None,
              help="Show posts before this ISO timestamp (timeline lens).")
@click.option("--query", "-q", default=None,
              help="Text search query (search lens).")
@click.option("--author", default=None,
              help="Filter by author handle (search lens).")
@click.option("--top", "-n", default=None, type=int,
              help="Number of results (threads/highlights lens).")
def show(web_id: str, lens: str, **kwargs):
    """Render a stored context web through a lens.

    WEB_ID is the identifier printed by 'fetch', or a unique prefix.
    """
    try:
        web = load_web(web_id)
    except (FileNotFoundError, ValueError) as e:
        click.echo(f"Error: {e}", err=True)
        sys.exit(1)

    params = {k: v for k, v in kwargs.items() if v is not None}
    click.echo(render(web, lens, **params))


@main.command(name="list")
def list_cmd():
    """List all stored context webs."""
    webs = list_webs()
    if not webs:
        click.echo("No stored context webs.", err=True)
        return
    for w in webs:
        click.echo(f"{w['id']}  {w['nodes']} posts  {w.get('threads', '?')} threads  {w['crawled_at']}")
        click.echo(f"  {w['root_uri']}")


@main.group()
def auth():
    """Manage Bluesky credentials."""


@auth.command()
@click.option("--handle", prompt="Bluesky handle")
@click.option("--app-password", prompt="App password", hide_input=True)
def login(handle: str, app_password: str):
    """Store Bluesky credentials for API access."""
    from bsky_context.auth import load_config, save_config

    config = load_config()
    config["handle"] = handle
    config["app_password"] = app_password
    config.pop("session", None)  # clear stale session
    save_config(config)
    click.echo("Credentials saved.", err=True)

    # Verify by attempting login
    async def _verify():
        from bsky_context.auth import get_client

        try:
            client = await get_client()
            click.echo(f"Authenticated as @{client.me.handle}", err=True)
        except Exception as e:
            click.echo(f"Warning: login verification failed: {e}", err=True)
            click.echo("Credentials saved anyway — check handle/password.", err=True)

    asyncio.run(_verify())
