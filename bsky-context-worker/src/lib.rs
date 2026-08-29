//! Cloudflare Worker adapter: crawls a context web per request and returns
//! it as markdown, so a language model with a URL fetcher can read a whole
//! Bluesky conversation from one link.
//!
//! Crawls are bounded per request (see the `CRAWL_*` variables in
//! `wrangler.toml`) and cached in the `WEBS` KV namespace when it is bound,
//! so repeated fetches of the same URL continue where the last one stopped.

mod client;
pub mod route;

use core::time::Duration;

use bsky_context_core::crawler::{CrawlOptions, CrawlResult, crawl};
use bsky_context_core::model::ContextWeb;
use worker::{Context, Env, Request, Response, Result, event, kv::KvStore};

use crate::client::{WorkerClock, WorkerFetch};
use crate::route::{Format, Route, RouteError, WebQuery, cache_key, cached_web_matches, compose};

const KV_BINDING: &str = "WEBS";
const CACHE_TTL_SECS: u64 = 60 * 60 * 24 * 30;

#[event(start)]
fn start() {
    console_error_panic_hook::set_once();
}

#[event(fetch)]
async fn fetch(req: Request, env: Env, _ctx: Context) -> Result<Response> {
    let url = req.url()?;
    match route::parse(&url) {
        Ok(Route::Home) => markdown(route::USAGE),
        Ok(Route::Web(query)) => serve_web(query, &env).await,
        Err(RouteError::NotFound) => Response::error("not found", 404),
        Err(RouteError::BadRequest(message)) => Response::error(message, 400),
    }
}

fn markdown(body: &str) -> Result<Response> {
    let response = Response::ok(body)?;
    response
        .headers()
        .set("content-type", "text/markdown; charset=utf-8")?;
    Ok(response)
}

fn budget(env: &Env) -> CrawlOptions {
    let number = |name: &str| {
        env.var(name)
            .ok()
            .and_then(|v| v.to_string().parse::<u64>().ok())
    };
    let defaults = CrawlOptions::default();
    CrawlOptions {
        max_nodes: number("CRAWL_MAX_NODES").map_or(500, |n| n.try_into().unwrap_or(usize::MAX)),
        max_depth: number("CRAWL_MAX_DEPTH").map(|n| n.try_into().unwrap_or(usize::MAX)),
        timeout: number("CRAWL_TIMEOUT_SECS").map_or(Duration::from_secs(20), Duration::from_secs),
        concurrency: number("CRAWL_CONCURRENCY")
            .map_or(4, |n| n.try_into().unwrap_or(defaults.concurrency)),
    }
}

async fn load_cached(kv: Option<&KvStore>, query: &WebQuery) -> Option<ContextWeb> {
    if query.fresh {
        return None;
    }
    let json = kv?.get(&cache_key(&query.post)).text().await.ok()??;
    let web = ContextWeb::from_json(&json).ok()?;
    cached_web_matches(&web, &query.post).then_some(web)
}

async fn store(kv: Option<&KvStore>, query: &WebQuery, web: &ContextWeb) {
    let Some(kv) = kv else {
        return;
    };
    let Ok(put) = kv.put(&cache_key(&query.post), web.to_json_pretty()) else {
        return;
    };
    if let Err(err) = put.expiration_ttl(CACHE_TTL_SECS).execute().await {
        log::warn!("KV put failed: {err}");
    }
}

async fn serve_web(query: WebQuery, env: &Env) -> Result<Response> {
    let kv = env.kv(KV_BINDING).ok();
    let options = budget(env);
    let existing = load_cached(kv.as_ref(), &query).await;

    let fetch = WorkerFetch::public();
    let clock = WorkerClock::new();
    let CrawlResult {
        web,
        stop_reason,
        pending,
    } = crawl(
        &fetch,
        &clock,
        &query.post.at_uri(),
        &options,
        existing,
        &mut |_| {},
    )
    .await;

    if web.node_count() == 0 {
        return Response::error("post not found or not fetchable", 404);
    }
    store(kv.as_ref(), &query, &web).await;

    let response = match query.format {
        Format::Json => Response::from_json(&web)?,
        Format::Markdown => markdown(&compose(&web, &query.lens, stop_reason, pending, &options))?,
    };
    response
        .headers()
        .set("cache-control", "public, max-age=60")?;
    Ok(response)
}
