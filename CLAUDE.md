# bsky-context

Bluesky Context Web crawler — fetches the full DAG of replies + quote posts for a Bluesky conversation.

## Development

- Rust workspace (Cargo). Run the CLI: `cargo run -p bsky-context-cli -- <command>`
- Test: `cargo test --workspace`; lint: `cargo clippy --workspace --all-targets -- -D warnings` (pedantic); format: `cargo fmt --all`
- Web app dev loop: `wasm-pack build bsky-context-web --target web --out-dir ../web/pkg`, then serve `web/` with any static server
- Worker dev loop: `cd bsky-context-worker && npx wrangler dev`
- Rust style: `rust-style` and `rustdoc` skills; WASM/Pages invariants: `wasm-pages-app` skill
- The Python package under `src/` and `tests/` is the original implementation the Rust port was written from. It is kept only as a reference until the port is verified; the Rust CLI reads the same storage directory.

## Architecture

**Thread Web**: Conversations are modeled as a collection of threads (reply trees) linked by quote edges. Each thread is the atomic crawl unit (one `getPostThread` call). Stored as JSON (`format_version` 2), rendered through lenses on demand.

**Pure core, thin adapters**: all logic lives in `bsky-context-core`, which does no I/O. HTTP and time come in through the `Fetch` and `Clock` traits (`api.rs`), so the same crawler runs natively, in a browser, and in a Cloudflare Worker.

- `bsky-context-core/` — `model.rs` (Post, Thread, QuoteEdge, ContextWeb, storage JSON), `uri.rs` (AT URI / bsky.app URL parsing), `api.rs` (AppView wire types, `Fetch`/`Clock` traits), `crawler.rs` (thread-level BFS with dedup and smart re-fetch), `lens/` (one file per lens, shared helpers in `mod.rs`)
- `bsky-context-cli/` — `bsky-context` binary: `fetch`, `show`, `list`; stores webs in `~/.local/share/bsky-context/webs/`
- `bsky-context-web/` + `web/` — wasm-bindgen bridge and framework-free page, deployed to GitHub Pages on version tags; crawls client-side against the public AppView
- `bsky-context-worker/` — Cloudflare Worker serving `/t/<handle>/<rkey>` as markdown for URL-fetching language models; bounded per-request crawl, optional KV cache

## Bluesky API notes

- Only two endpoints: `app.bsky.feed.getPostThread` and `app.bsky.feed.getQuotes`, both served unauthenticated with CORS by `https://public.api.bsky.app`
- The `!no-unauthenticated` profile label is advisory: the public API still returns those posts (verified 2026-08-29). Honoring it is a client decision.
- Rate limit: ~3000 req / 5 min per IP
