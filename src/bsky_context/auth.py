"""Credential management for Bluesky authentication."""

from __future__ import annotations

import json
import os
from pathlib import Path


def get_config_dir() -> Path:
    xdg = os.environ.get("XDG_CONFIG_HOME")
    base = Path(xdg) if xdg else Path.home() / ".config"
    return base / "bsky-context"


def load_config() -> dict:
    config_file = get_config_dir() / "config.json"
    if config_file.exists():
        return json.loads(config_file.read_text())
    return {}


def save_config(config: dict) -> None:
    config_dir = get_config_dir()
    config_dir.mkdir(parents=True, exist_ok=True)
    config_file = config_dir / "config.json"
    config_file.write_text(json.dumps(config, indent=2))
    config_file.chmod(0o600)


async def get_client():
    """Return an authenticated AsyncClient, using cached session if available."""
    from atproto import AsyncClient

    config = load_config()
    client = AsyncClient()

    # Try session string first
    session_string = config.get("session")
    if session_string:
        try:
            await client.login(session_string=session_string)
            _register_session_handler(client)
            return client
        except Exception:
            pass  # Session expired, fall through

    handle = config.get("handle")
    app_password = config.get("app_password")
    if not handle or not app_password:
        raise RuntimeError(
            "No credentials configured. Run: bsky-context auth login"
        )

    await client.login(handle, app_password)
    # Persist session for next time
    config["session"] = client.export_session_string()
    save_config(config)
    _register_session_handler(client)
    return client


def _register_session_handler(client) -> None:
    """Persist session refreshes to disk automatically."""
    from atproto import SessionEvent

    @client.on_session_change
    async def _on_session_change(event, session):
        if event in (SessionEvent.CREATE, SessionEvent.REFRESH):
            config = load_config()
            config["session"] = client.export_session_string()
            save_config(config)
