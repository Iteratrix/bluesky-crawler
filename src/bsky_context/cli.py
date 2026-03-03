"""CLI entry point for bsky-context."""

from __future__ import annotations

import asyncio
import sys

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
              help="Ignore any stored version and crawl from scratch.")
def fetch(post_url: str, max_nodes: int, max_depth: int | None, timeout: float,
          fresh: bool):
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

    async def _run():
        from bsky_context.auth import get_client
        from bsky_context.crawler import crawl
        from bsky_context.storage import web_id

        try:
            client = await get_client()
        except RuntimeError as e:
            click.echo(f"Error: {e}", err=True)
            sys.exit(1)

        # Auto-load existing web unless --fresh
        existing = None
        if not fresh:
            try:
                existing = load_web(web_id(ref.at_uri))
                click.echo(
                    f"  Updating existing web ({existing.node_count} posts)...",
                    err=True,
                )
            except FileNotFoundError:
                pass  # No existing web, fresh crawl

        def _progress(nodes: int, edges: int) -> None:
            click.echo(f"\r  Crawling... {nodes} posts, {edges} edges", nl=False, err=True)

        web = await crawl(
            client,
            ref.at_uri,
            max_nodes=max_nodes,
            max_depth=max_depth,
            timeout=timeout,
            existing=existing,
            progress_callback=_progress,
        )

        path = save_web(web)
        click.echo("", err=True)  # newline after progress
        click.echo(f"  Saved: {path.stem}", err=True)
        click.echo(f"  {web.node_count} posts, {web.edge_count} edges", err=True)
        # Machine-consumable output: just the web ID
        click.echo(path.stem)

    asyncio.run(_run())


@main.command()
@click.argument("web_id")
@click.option("--lens", "-l", default="tree",
              type=click.Choice(["tree", "linear", "by-author", "raw"]),
              help="View to render.")
def show(web_id: str, lens: str):
    """Render a stored context web through a lens.

    WEB_ID is the identifier printed by 'fetch', or a unique prefix.
    """
    try:
        web = load_web(web_id)
    except (FileNotFoundError, ValueError) as e:
        click.echo(f"Error: {e}", err=True)
        sys.exit(1)

    click.echo(render(web, lens))


@main.command(name="list")
def list_cmd():
    """List all stored context webs."""
    webs = list_webs()
    if not webs:
        click.echo("No stored context webs.", err=True)
        return
    for w in webs:
        click.echo(f"{w['id']}  {w['nodes']} posts  {w['crawled_at']}")
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
