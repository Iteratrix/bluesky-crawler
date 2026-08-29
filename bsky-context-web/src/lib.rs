//! WASM bridge: JSON-in/JSON-out API over the core crate.
//!
//! Thin by design: (de)serialization lives here, logic lives in core. A
//! string API keeps the JS side trivial (`JSON.parse` and go).

mod client;

use bsky_context_core::crawler::{CrawlOptions, CrawlResult, Progress, StopReason, crawl};
use bsky_context_core::lens::{LensKind, LensParams, render};
use bsky_context_core::model::{ContextWeb, web_id};
use bsky_context_core::uri::PostRef;
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

use crate::client::{BrowserClock, BrowserFetch};

#[wasm_bindgen(start)]
fn start() {
    console_error_panic_hook::set_once();
    let _ = console_log::init_with_level(log::Level::Info);
}

/// Normalizes an AT URI or bsky.app URL to a canonical AT URI.
///
/// # Errors
///
/// Returns a JS error when the input is neither form.
#[wasm_bindgen]
pub fn parse_post_ref(input: &str) -> Result<String, JsError> {
    Ok(PostRef::parse(input)?.at_uri())
}

/// Returns the storage identifier for a web rooted at `root_uri`.
#[wasm_bindgen(js_name = webId)]
#[must_use]
pub fn web_id_js(root_uri: &str) -> String {
    web_id(root_uri)
}

#[derive(Deserialize)]
#[serde(default)]
struct CrawlOptionsJson {
    max_nodes: usize,
    max_depth: Option<usize>,
    timeout_secs: f64,
    concurrency: usize,
}

impl Default for CrawlOptionsJson {
    fn default() -> Self {
        let CrawlOptions {
            max_nodes,
            max_depth,
            timeout,
            concurrency,
        } = CrawlOptions::default();
        Self {
            max_nodes,
            max_depth,
            timeout_secs: timeout.as_secs_f64(),
            concurrency,
        }
    }
}

impl From<CrawlOptionsJson> for CrawlOptions {
    fn from(json: CrawlOptionsJson) -> Self {
        let CrawlOptionsJson {
            max_nodes,
            max_depth,
            timeout_secs,
            concurrency,
        } = json;
        Self {
            max_nodes,
            max_depth,
            timeout: core::time::Duration::from_secs_f64(timeout_secs),
            concurrency,
        }
    }
}

#[derive(Serialize)]
struct ProgressJson {
    nodes: usize,
    edges: usize,
    threads: usize,
}

#[derive(Serialize)]
struct CrawlResultJson {
    web: ContextWeb,
    stop_reason: &'static str,
    pending: usize,
}

fn stop_reason_name(reason: StopReason) -> &'static str {
    match reason {
        StopReason::Complete => "complete",
        StopReason::MaxNodes => "max_nodes",
        StopReason::Timeout => "timeout",
    }
}

/// Crawls the context web of a post from the browser.
///
/// `options_json` is `{max_nodes, max_depth, timeout_secs, concurrency}`
/// with every field optional; `existing_json` is a previously returned web
/// to update incrementally. `on_progress` receives
/// `{nodes, edges, threads}` objects. Resolves to
/// `{web, stop_reason, pending}` as JSON text.
///
/// # Errors
///
/// Returns a JS error for a malformed post reference, options, or
/// existing web.
#[wasm_bindgen]
pub async fn crawl_web(
    post: String,
    options_json: String,
    existing_json: Option<String>,
    on_progress: js_sys::Function,
) -> Result<String, JsError> {
    let post_ref = PostRef::parse(&post)?;
    let options: CrawlOptionsJson = if options_json.trim().is_empty() {
        CrawlOptionsJson::default()
    } else {
        serde_json::from_str(&options_json)?
    };
    let options = CrawlOptions::from(options);
    let existing = match existing_json {
        Some(json) => Some(ContextWeb::from_json(&json)?),
        None => None,
    };
    let fetch = BrowserFetch::public();
    let clock = BrowserClock::new();
    let mut report = |progress: Progress| {
        let Progress {
            node_count,
            edge_count,
            thread_count,
        } = progress;
        let payload = ProgressJson {
            nodes: node_count,
            edges: edge_count,
            threads: thread_count,
        };
        if let Ok(value) = serde_wasm_value(&payload) {
            let _ = on_progress.call1(&JsValue::NULL, &value);
        }
    };
    let CrawlResult {
        web,
        stop_reason,
        pending,
    } = crawl(
        &fetch,
        &clock,
        &post_ref.at_uri(),
        &options,
        existing,
        &mut report,
    )
    .await;
    let result = CrawlResultJson {
        web,
        stop_reason: stop_reason_name(stop_reason),
        pending,
    };
    Ok(serde_json::to_string(&result)?)
}

fn serde_wasm_value<T: Serialize>(value: &T) -> Result<JsValue, JsError> {
    let json = serde_json::to_string(value)?;
    js_sys::JSON::parse(&json).map_err(|_| JsError::new("progress payload is not valid JSON"))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct LensParamsJson {
    top: Option<usize>,
    hops: Option<usize>,
    uri: Option<String>,
    after: Option<String>,
    before: Option<String>,
    query: Option<String>,
    author: Option<String>,
}

impl From<LensParamsJson> for LensParams {
    fn from(json: LensParamsJson) -> Self {
        let LensParamsJson {
            top,
            hops,
            uri,
            after,
            before,
            query,
            author,
        } = json;
        Self {
            top,
            hops,
            uri: uri.filter(|s| !s.is_empty()),
            after: after.filter(|s| !s.is_empty()),
            before: before.filter(|s| !s.is_empty()),
            query: query.filter(|s| !s.is_empty()),
            author: author.filter(|s| !s.is_empty()),
        }
    }
}

/// Renders a web (as JSON text) through a lens.
///
/// `lens` is a lens name (`tree`, `linear`, ...); `params_json` is
/// `{top, hops, uri, after, before, query, author}` with every field
/// optional.
///
/// # Errors
///
/// Returns a JS error for an unknown lens or malformed input.
#[wasm_bindgen]
pub fn render_web(web_json: &str, lens: &str, params_json: &str) -> Result<String, JsError> {
    let web = ContextWeb::from_json(web_json)?;
    let kind: LensKind = lens.parse()?;
    let params: LensParamsJson = if params_json.trim().is_empty() {
        LensParamsJson::default()
    } else {
        serde_json::from_str(params_json)?
    };
    Ok(render(&web, &kind.with_params(&params.into())))
}

/// Returns the lens names the page can offer.
#[wasm_bindgen(js_name = lensNames)]
#[must_use]
pub fn lens_names() -> Vec<String> {
    bsky_context_core::lens::NAMES
        .iter()
        .map(|s| (*s).to_owned())
        .collect()
}
