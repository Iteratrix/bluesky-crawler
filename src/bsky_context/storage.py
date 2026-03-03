"""Local storage for crawled context webs."""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path

from bsky_context.models import ContextWeb


def get_data_dir() -> Path:
    xdg = os.environ.get("XDG_DATA_HOME")
    base = Path(xdg) if xdg else Path.home() / ".local" / "share"
    return base / "bsky-context" / "webs"


def web_id(root_uri: str) -> str:
    """Generate a short deterministic ID from a root URI.

    Format: {rkey}-{sha256_prefix} for readability + collision resistance.
    """
    rkey = root_uri.rsplit("/", 1)[-1]
    h = hashlib.sha256(root_uri.encode()).hexdigest()[:6]
    return f"{rkey}-{h}"


def save_web(web: ContextWeb) -> Path:
    data_dir = get_data_dir()
    data_dir.mkdir(parents=True, exist_ok=True)
    wid = web_id(web.root_uri)
    path = data_dir / f"{wid}.json"
    path.write_text(json.dumps(web.to_dict(), indent=2, ensure_ascii=False))
    return path


def load_web(identifier: str) -> ContextWeb:
    """Load a ContextWeb by ID or prefix match."""
    data_dir = get_data_dir()
    # Exact match
    exact = data_dir / f"{identifier}.json"
    if exact.exists():
        return ContextWeb.from_dict(json.loads(exact.read_text()))
    # Prefix match
    matches = sorted(data_dir.glob(f"{identifier}*.json"))
    if len(matches) == 1:
        return ContextWeb.from_dict(json.loads(matches[0].read_text()))
    if len(matches) > 1:
        names = [m.stem for m in matches]
        raise ValueError(f"Ambiguous ID '{identifier}', matches: {names}")
    raise FileNotFoundError(f"No web found for '{identifier}'")


def list_webs() -> list[dict]:
    data_dir = get_data_dir()
    if not data_dir.exists():
        return []
    result = []
    for path in sorted(data_dir.glob("*.json")):
        data = json.loads(path.read_text())
        meta = data.get("meta", {})
        result.append({
            "id": path.stem,
            "root_uri": meta.get("root_uri", "?"),
            "crawled_at": meta.get("crawled_at", "?"),
            "nodes": meta.get("node_count", 0),
            "edges": meta.get("edge_count", 0),
        })
    return result
