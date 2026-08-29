//! Cloudflare Worker adapter: an MCP server that crawls a Bluesky context
//! web per tool call and returns it as text, so a chat client with a
//! custom connector can read a whole conversation from one post URL.
//!
//! Crawls are bounded per call (see the `CRAWL_*` variables in
//! `wrangler.toml`) and cached in the `WEBS` KV namespace when it is bound:
//! a call within `CACHE_FRESH_SECS` of the last crawl renders the cached
//! web directly (cheap lens switching), later calls continue the crawl
//! from where it stopped.

mod cache;
mod client;
pub mod mcp;

use core::time::Duration;

use bsky_context_core::crawler::{CrawlOptions, CrawlResult, crawl};
use serde_json::Value;
use worker::{Context, Env, Method, Request, Response, Result, event, kv::KvStore};

use crate::cache::Envelope;
use crate::client::{WorkerClock, WorkerFetch};
use crate::mcp::{Provenance, Step, ToolArgs, compose, tool_error, tool_result};

const KV_BINDING: &str = "WEBS";
const CACHE_TTL_SECS: u64 = 60 * 60 * 24 * 30;

#[event(start)]
fn start() {
    console_error_panic_hook::set_once();
    let _ = console_log::init_with_level(log::Level::Info);
}

#[event(fetch)]
async fn fetch(mut req: Request, env: Env, _ctx: Context) -> Result<Response> {
    let path = req.path();
    let path = path.trim_end_matches('/');
    match (req.method(), path) {
        (Method::Get, "") => markdown(mcp::USAGE),
        (Method::Post, "/mcp") => serve_mcp(&mut req, &env).await,
        (
            Method::Get
            | Method::Head
            | Method::Put
            | Method::Delete
            | Method::Options
            | Method::Connect
            | Method::Patch
            | Method::Trace,
            "/mcp",
        ) => Response::error("use POST for MCP over Streamable HTTP", 405),
        _ => Response::error("not found", 404),
    }
}

fn markdown(body: &str) -> Result<Response> {
    let response = Response::ok(body)?;
    response
        .headers()
        .set("content-type", "text/markdown; charset=utf-8")?;
    Ok(response)
}

fn env_number(env: &Env, name: &str) -> Option<u64> {
    env.var(name).ok()?.to_string().parse().ok()
}

fn budget(env: &Env) -> CrawlOptions {
    let defaults = CrawlOptions::default();
    CrawlOptions {
        max_nodes: env_number(env, "CRAWL_MAX_NODES")
            .map_or(500, |n| n.try_into().unwrap_or(usize::MAX)),
        max_depth: env_number(env, "CRAWL_MAX_DEPTH").map(|n| n.try_into().unwrap_or(usize::MAX)),
        timeout: env_number(env, "CRAWL_TIMEOUT_SECS")
            .map_or(Duration::from_secs(20), Duration::from_secs),
        concurrency: env_number(env, "CRAWL_CONCURRENCY")
            .map_or(4, |n| n.try_into().unwrap_or(defaults.concurrency)),
    }
}

fn cache_fresh_window(env: &Env) -> u64 {
    env_number(env, "CACHE_FRESH_SECS").unwrap_or(300)
}

async fn serve_mcp(req: &mut Request, env: &Env) -> Result<Response> {
    let body = req.text().await?;
    let parsed = match mcp::parse_body(&body) {
        Ok(parsed) => parsed,
        Err(error) => return Ok(Response::from_json(&error)?.with_status(400)),
    };
    let mut replies = Vec::new();
    for message in &parsed.messages {
        match mcp::handle(message) {
            Step::Reply(value) => replies.push(value),
            Step::Ignore => {}
            Step::CallTool { id, args } => replies.push(run_tool(id, args, env).await),
        }
    }
    if replies.is_empty() {
        return Ok(Response::empty()?.with_status(202));
    }
    let payload = if parsed.batch {
        Value::Array(replies)
    } else {
        replies.swap_remove(0)
    };
    Response::from_json(&payload)
}

async fn load_cached(kv: Option<&KvStore>, args: &ToolArgs) -> Option<Envelope> {
    if args.fresh {
        return None;
    }
    let key = cache::key(&args.post);
    let json = match kv?.get(&key).text().await {
        Ok(json) => json?,
        Err(err) => {
            log::warn!("KV get {key} failed (crawling without cache): {err}");
            return None;
        }
    };
    let envelope: Envelope = serde_json::from_str(&json).ok()?;
    cache::matches(&envelope.web, &args.post).then_some(envelope)
}

async fn store(kv: Option<&KvStore>, args: &ToolArgs, envelope: &Envelope) {
    let Some(kv) = kv else {
        return;
    };
    let Ok(json) = serde_json::to_string(envelope) else {
        return;
    };
    let key = cache::key(&args.post);
    let put = match kv.put(&key, json) {
        Ok(put) => put,
        Err(err) => {
            log::warn!("KV put {key} could not be prepared: {err}");
            return;
        }
    };
    if let Err(err) = put.expiration_ttl(CACHE_TTL_SECS).execute().await {
        log::warn!("KV put {key} failed (cache disabled until it recovers): {err}");
    }
}

async fn run_tool(id: Value, args: ToolArgs, env: &Env) -> Value {
    let kv = env.kv(KV_BINDING).ok();
    let options = budget(env);
    let now_ms = js_sys::Date::now();

    let cached = load_cached(kv.as_ref(), &args).await;
    if let Some(envelope) = &cached
        && envelope.age_secs(now_ms) < cache_fresh_window(env)
    {
        let age_secs = envelope.age_secs(now_ms);
        let text = compose(
            &envelope.web,
            &args.lens,
            Provenance::Cached { age_secs },
            &options,
        );
        return tool_result(&id, &text);
    }

    let fetch = WorkerFetch::public();
    let clock = WorkerClock::new();
    let CrawlResult {
        web,
        stop_reason,
        pending,
    } = crawl(
        &fetch,
        &clock,
        &args.post.at_uri(),
        &options,
        cached.map(|e| e.web),
        &mut |_| {},
    )
    .await;

    if web.node_count() == 0 {
        return tool_error(
            &id,
            &format!(
                "No posts found for {}: the post may not exist, or Bluesky may be unreachable.",
                args.post.at_uri()
            ),
        );
    }
    let envelope = Envelope {
        stored_ms: js_sys::Date::now(),
        web,
    };
    store(kv.as_ref(), &args, &envelope).await;
    let text = compose(
        &envelope.web,
        &args.lens,
        Provenance::Crawled {
            stop_reason,
            pending,
        },
        &options,
    );
    tool_result(&id, &text)
}
