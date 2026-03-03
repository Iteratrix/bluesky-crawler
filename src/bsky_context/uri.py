"""Parse and normalize Bluesky post identifiers."""

from __future__ import annotations

import re
from dataclasses import dataclass

AT_URI_RE = re.compile(
    r"^at://(?P<repo>[^/]+)/app\.bsky\.feed\.post/(?P<rkey>[a-zA-Z0-9]+)$"
)

BSKY_URL_RE = re.compile(
    r"^https?://bsky\.app/profile/(?P<handle>[^/]+)/post/(?P<rkey>[a-zA-Z0-9]+)$"
)


@dataclass(frozen=True)
class PostRef:
    """A reference to a Bluesky post."""

    repo: str  # DID or handle
    rkey: str  # Record key

    @property
    def at_uri(self) -> str:
        return f"at://{self.repo}/app.bsky.feed.post/{self.rkey}"

    @classmethod
    def from_str(cls, s: str) -> PostRef:
        """Parse an AT URI or bsky.app URL into a PostRef."""
        s = s.strip()
        m = AT_URI_RE.match(s)
        if m:
            return cls(repo=m.group("repo"), rkey=m.group("rkey"))
        m = BSKY_URL_RE.match(s)
        if m:
            return cls(repo=m.group("handle"), rkey=m.group("rkey"))
        raise ValueError(f"Cannot parse as Bluesky post reference: {s}")

    def __str__(self) -> str:
        return self.at_uri
