//! Model Context Protocol server logic, kept free of the `worker` runtime
//! so it can be unit tested natively.
//!
//! Implements the subset of MCP a stateless Streamable HTTP server needs:
//! `initialize`, `ping`, `tools/list`, and `tools/call` for the single
//! `bsky_context` tool. Every request arrives as one JSON-RPC message (or a
//! batch) in a POST body; [`handle`] turns each into a [`Step`] and the
//! runtime performs any crawl a step asks for.

use bsky_context_core::crawler::{CrawlOptions, StopReason};
use bsky_context_core::lens::{Lens, LensKind, LensParams, NAMES, render};
use bsky_context_core::model::ContextWeb;
use bsky_context_core::uri::{PostRef, rkey_of};
use serde::Deserialize;
use serde_json::{Value, json};

/// The newest protocol revision this server speaks.
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// Protocol revisions the server accepts from a client, newest first.
const SUPPORTED_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

/// Name of the one tool the server exposes.
pub const TOOL_NAME: &str = "bsky_context";

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

/// A parsed POST body: the messages it held and whether it was a batch.
#[derive(Debug, Clone, PartialEq)]
pub struct Body {
    /// The JSON-RPC messages, in order.
    pub messages: Vec<Value>,
    /// Whether the body was a JSON array; the reply mirrors the shape.
    pub batch: bool,
}

/// Parses a POST body into JSON-RPC messages.
///
/// # Errors
///
/// Returns a ready-to-send JSON-RPC error response when the body is not
/// valid JSON or is neither an object nor a non-empty array of objects.
pub fn parse_body(body: &str) -> Result<Body, Value> {
    let value: Value = serde_json::from_str(body)
        .map_err(|e| error_response(&Value::Null, PARSE_ERROR, &format!("parse error: {e}")))?;
    match value {
        Value::Object(_) => Ok(Body {
            messages: vec![value],
            batch: false,
        }),
        Value::Array(items) if !items.is_empty() && items.iter().all(Value::is_object) => {
            Ok(Body {
                messages: items,
                batch: true,
            })
        }
        Value::Array(_) | Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {
            Err(error_response(
                &Value::Null,
                INVALID_REQUEST,
                "expected a JSON-RPC object or batch",
            ))
        }
    }
}

/// What the runtime should do with one JSON-RPC message.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// Send this response as is.
    Reply(Value),
    /// Run the tool, then send [`tool_result`] or [`tool_error`] under `id`.
    CallTool {
        /// The request id to answer under.
        id: Value,
        /// The validated tool arguments.
        args: ToolArgs,
    },
    /// A notification; nothing to send.
    Ignore,
}

/// Validated arguments of a `bsky_context` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolArgs {
    /// The starting post.
    pub post: PostRef,
    /// The lens to render with.
    pub lens: Lens,
    /// Whether to ignore any cached web and crawl from scratch.
    pub fresh: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawArgs {
    post: String,
    #[serde(default)]
    lens: Option<String>,
    #[serde(default)]
    top: Option<usize>,
    #[serde(default)]
    hops: Option<usize>,
    #[serde(default)]
    uri: Option<String>,
    #[serde(default)]
    after: Option<String>,
    #[serde(default)]
    before: Option<String>,
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    fresh: Option<bool>,
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|s| !s.trim().is_empty())
}

impl TryFrom<Value> for ToolArgs {
    type Error = String;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        let RawArgs {
            post,
            lens,
            top,
            hops,
            uri,
            after,
            before,
            query,
            author,
            fresh,
        } = serde_json::from_value(value).map_err(|e| e.to_string())?;
        let post = PostRef::parse(&post).map_err(|e| e.to_string())?;
        let kind: LensKind = lens
            .as_deref()
            .filter(|s| !s.is_empty())
            .unwrap_or("tree")
            .parse()
            .map_err(|e: bsky_context_core::lens::UnknownLens| e.to_string())?;
        let params = LensParams {
            top,
            hops,
            uri: non_empty(uri),
            after: non_empty(after),
            before: non_empty(before),
            query: non_empty(query),
            author: non_empty(author),
        };
        Ok(Self {
            post,
            lens: kind.with_params(&params),
            fresh: fresh.unwrap_or(false),
        })
    }
}

/// Decides what to do with one JSON-RPC message.
#[must_use]
pub fn handle(message: &Value) -> Step {
    let id = message.get("id").cloned();
    let method = message.get("method").and_then(Value::as_str).unwrap_or("");
    let Some(id) = id else {
        return Step::Ignore;
    };
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    match method {
        "initialize" => Step::Reply(success(&id, &initialize_result(&params))),
        "ping" => Step::Reply(success(&id, &json!({}))),
        "tools/list" => Step::Reply(success(&id, &json!({ "tools": [tool_definition()] }))),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            if name != TOOL_NAME {
                return Step::Reply(error_response(
                    &id,
                    INVALID_PARAMS,
                    &format!("unknown tool {name:?}"),
                ));
            }
            let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
            match ToolArgs::try_from(arguments) {
                Ok(args) => Step::CallTool { id, args },
                Err(message) => Step::Reply(error_response(&id, INVALID_PARAMS, &message)),
            }
        }
        _ => Step::Reply(error_response(
            &id,
            METHOD_NOT_FOUND,
            &format!("method not found: {method}"),
        )),
    }
}

fn initialize_result(params: &Value) -> Value {
    let requested = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or(PROTOCOL_VERSION);
    let version = if SUPPORTED_VERSIONS.contains(&requested) {
        requested
    } else {
        PROTOCOL_VERSION
    };
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": {
            "name": "bsky-context",
            "version": env!("CARGO_PKG_VERSION"),
        },
        "instructions": INSTRUCTIONS,
    })
}

const INSTRUCTIONS: &str = "\
Use the bsky_context tool whenever a Bluesky post (bsky.app URL or at:// URI) \
comes up and you need to know what the conversation around it says. It returns \
the whole conversation graph, replies and quote posts recursively, as text. For \
large conversations call it with lens=stats first, then narrow down with \
highlights, neighborhood, or search.";

const TOOL_DESCRIPTION: &str = "\
Fetch the full conversation around a Bluesky post: the reply tree plus every \
post that quotes it, recursively (a \"context web\"), rendered as text through \
a lens.

Lenses:
- tree (default): indented threaded view; who replied to whom
- linear: chronological narrative with cross-references; best for summarizing
- by-author: posts grouped by participant; best for analyzing a debate
- stats: post/thread counts, top authors, engagement, depth; use first on big webs
- threads: sub-conversations sorted by size (top)
- highlights: most quoted, most replied, highest engagement posts (top)
- neighborhood: posts within N quote hops of a post (hops, uri)
- timeline: posts within a time window (after, before; ISO 8601)
- search: posts matching text and/or author handle (query, author)
- raw: the full JSON graph

Crawls are bounded per call. When the result says the budget was reached, call \
again with the same post to continue from the cached web. Results are cached \
briefly, so switching lenses on the same post is cheap; pass fresh=true to force \
a re-crawl.";

fn tool_definition() -> Value {
    json!({
        "name": TOOL_NAME,
        "title": "Bluesky conversation context",
        "description": TOOL_DESCRIPTION,
        "inputSchema": {
            "type": "object",
            "properties": {
                "post": {
                    "type": "string",
                    "description": "A bsky.app post URL (https://bsky.app/profile/<handle>/post/<rkey>) or an at:// post URI",
                },
                "lens": {
                    "type": "string",
                    "enum": NAMES,
                    "default": "tree",
                    "description": "Which rendering to return",
                },
                "top": { "type": "integer", "minimum": 1, "description": "Result count for threads and highlights" },
                "hops": { "type": "integer", "minimum": 0, "description": "Quote-hop radius for neighborhood (default 2)" },
                "uri": { "type": "string", "description": "Center post (at:// URI) for neighborhood; defaults to the starting post" },
                "after": { "type": "string", "description": "Inclusive lower bound for timeline, ISO 8601" },
                "before": { "type": "string", "description": "Exclusive upper bound for timeline, ISO 8601" },
                "query": { "type": "string", "description": "Case-insensitive text to search for (search lens)" },
                "author": { "type": "string", "description": "Handle substring to filter by (search lens)" },
                "fresh": { "type": "boolean", "default": false, "description": "Ignore the cached web and crawl from scratch" },
            },
            "required": ["post"],
        },
        "annotations": {
            "readOnlyHint": true,
            "openWorldHint": true,
        },
    })
}

fn success(id: &Value, result: &Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

/// Builds a JSON-RPC error response.
#[must_use]
pub fn error_response(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// Builds a successful `tools/call` response carrying `text`.
#[must_use]
pub fn tool_result(id: &Value, text: &str) -> Value {
    success(
        id,
        &json!({ "content": [{ "type": "text", "text": text }], "isError": false }),
    )
}

/// Builds a `tools/call` response reporting a tool execution failure.
#[must_use]
pub fn tool_error(id: &Value, message: &str) -> Value {
    success(
        id,
        &json!({ "content": [{ "type": "text", "text": message }], "isError": true }),
    )
}

/// How the web being rendered was obtained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provenance {
    /// A crawl ran (possibly continuing a cached web) and stopped for this reason.
    Crawled {
        /// Why the crawl stopped.
        stop_reason: StopReason,
        /// Work items left when it stopped.
        pending: usize,
    },
    /// The cached web was rendered without crawling.
    Cached {
        /// Age of the cached web in whole seconds.
        age_secs: u64,
    },
}

/// Composes the tool's text result: a short header, then the lens output.
#[must_use]
pub fn compose(
    web: &ContextWeb,
    lens: &Lens,
    provenance: Provenance,
    budget: &CrawlOptions,
) -> String {
    let root_link = web.get_post(&web.root_uri).map_or_else(
        || web.root_uri.clone(),
        |p| {
            format!(
                "https://bsky.app/profile/{}/post/{}",
                p.author.handle,
                rkey_of(&p.uri)
            )
        },
    );
    let note = match provenance {
        Provenance::Crawled {
            stop_reason: StopReason::Complete,
            ..
        } => "The graph was fully explored.".to_owned(),
        Provenance::Crawled {
            stop_reason: StopReason::MaxNodes,
            pending,
        } => format!(
            "Crawl budget reached ({} posts per call); {pending} threads unexplored. \
             Call again with the same post to continue from the cached web.",
            budget.max_nodes
        ),
        Provenance::Crawled {
            stop_reason: StopReason::Timeout,
            pending,
        } => format!(
            "Crawl budget reached ({}s per call); {pending} threads unexplored. \
             Call again with the same post to continue from the cached web.",
            budget.timeout.as_secs()
        ),
        Provenance::Cached { age_secs } => {
            format!("Rendered from the cached web ({age_secs}s old); pass fresh=true to re-crawl.")
        }
    };
    format!(
        "# Bluesky context web: {root_link}\n\n\
         Crawled {}: {} posts, {} threads, {} edges. Lens: {}.\n\
         {note}\n\n---\n\n{}",
        web.crawled_at,
        web.node_count(),
        web.thread_count(),
        web.edge_count(),
        lens.kind(),
        render(web, lens)
    )
}

/// Usage text served at `/`.
pub const USAGE: &str = "\
# bsky-context

An MCP server that fetches the full conversation graph around a Bluesky post: \
the reply tree and every quote post, recursively, rendered as text.

Connect a client to `/mcp` (Streamable HTTP, no authentication) and call the \
`bsky_context` tool with a bsky.app post URL. In claude.ai: Settings -> \
Connectors -> Add custom connector -> paste this origin plus `/mcp`.

Source: https://github.com/Iteratrix/bluesky-crawler
";

#[cfg(test)]
mod tests {
    use core::time::Duration;

    use bsky_context_core::model::{Author, Post, Thread};

    use super::*;

    fn request(id: u64, method: &str, params: &Value) -> Value {
        json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
    }

    fn reply(step: Step) -> Value {
        let Step::Reply(value) = step else {
            panic!("expected a reply, got {step:?}");
        };
        value
    }

    #[test]
    fn parses_single_and_batch_bodies() {
        let single = parse_body(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#).unwrap();
        assert!(!single.batch);
        assert_eq!(single.messages.len(), 1);
        let batch = parse_body(r#"[{"jsonrpc":"2.0","id":1,"method":"ping"},{"jsonrpc":"2.0","method":"notifications/initialized"}]"#).unwrap();
        assert!(batch.batch);
        assert_eq!(batch.messages.len(), 2);
    }

    #[test]
    fn rejects_malformed_bodies() {
        let err = parse_body("{not json").unwrap_err();
        assert_eq!(err["error"]["code"], PARSE_ERROR);
        assert_eq!(err["id"], Value::Null);
        for body in ["[]", "42", "\"x\"", "[1]"] {
            let err = parse_body(body).unwrap_err();
            assert_eq!(err["error"]["code"], INVALID_REQUEST, "{body}");
        }
    }

    #[test]
    fn initialize_negotiates_version() {
        let out = reply(handle(&request(
            1,
            "initialize",
            &json!({ "protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": { "name": "t", "version": "0" } }),
        )));
        assert_eq!(out["id"], 1);
        assert_eq!(out["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(out["result"]["serverInfo"]["name"], "bsky-context");
        assert!(out["result"]["capabilities"]["tools"].is_object());
        assert!(
            out["result"]["instructions"]
                .as_str()
                .unwrap()
                .contains("bsky_context")
        );

        let out = reply(handle(&request(
            2,
            "initialize",
            &json!({ "protocolVersion": "1999-01-01" }),
        )));
        assert_eq!(out["result"]["protocolVersion"], PROTOCOL_VERSION);
    }

    #[test]
    fn notifications_are_ignored_and_ping_answers() {
        let note = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        assert_eq!(handle(&note), Step::Ignore);
        let out = reply(handle(&request(7, "ping", &Value::Null)));
        assert_eq!(out, json!({ "jsonrpc": "2.0", "id": 7, "result": {} }));
    }

    #[test]
    fn tools_list_describes_the_tool() {
        let out = reply(handle(&request(3, "tools/list", &json!({}))));
        let tools = out["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        let tool = &tools[0];
        assert_eq!(tool["name"], TOOL_NAME);
        assert_eq!(tool["inputSchema"]["required"], json!(["post"]));
        let lenses = tool["inputSchema"]["properties"]["lens"]["enum"]
            .as_array()
            .unwrap();
        assert_eq!(lenses.len(), NAMES.len());
        for name in NAMES {
            assert!(
                tool["description"].as_str().unwrap().contains(name),
                "{name}"
            );
        }
        assert_eq!(tool["annotations"]["readOnlyHint"], true);
    }

    #[test]
    fn tools_call_parses_arguments() {
        let step = handle(&request(
            4,
            "tools/call",
            &json!({ "name": TOOL_NAME, "arguments": {
                "post": "https://bsky.app/profile/alice.test/post/k1",
                "lens": "search", "query": "hello", "author": "", "fresh": true
            } }),
        ));
        assert_eq!(
            step,
            Step::CallTool {
                id: json!(4),
                args: ToolArgs {
                    post: PostRef {
                        repo: "alice.test".into(),
                        rkey: "k1".into()
                    },
                    lens: Lens::Search {
                        query: Some("hello".into()),
                        author: None
                    },
                    fresh: true,
                }
            }
        );

        let step = handle(&request(
            5,
            "tools/call",
            &json!({ "name": TOOL_NAME, "arguments": { "post": "at://did:plc:a/app.bsky.feed.post/k2", "lens": "threads", "top": 3 } }),
        ));
        let Step::CallTool { args, .. } = step else {
            panic!("expected CallTool");
        };
        assert_eq!(args.lens, Lens::Threads { top: 3 });
        assert!(!args.fresh);

        let step = handle(&request(
            6,
            "tools/call",
            &json!({ "name": TOOL_NAME, "arguments": { "post": "at://did:plc:a/app.bsky.feed.post/k2" } }),
        ));
        let Step::CallTool { args, .. } = step else {
            panic!("expected CallTool");
        };
        assert_eq!(args.lens, Lens::Tree);
    }

    #[test]
    fn tools_call_rejects_bad_arguments() {
        for arguments in [
            json!({}),
            json!({ "post": "https://example.com" }),
            json!({ "post": "at://did:plc:a/app.bsky.feed.post/k", "lens": "nope" }),
            json!({ "post": "at://did:plc:a/app.bsky.feed.post/k", "top": "many" }),
            json!({ "post": "at://did:plc:a/app.bsky.feed.post/k", "bogus": 1 }),
        ] {
            let out = reply(handle(&request(
                8,
                "tools/call",
                &json!({ "name": TOOL_NAME, "arguments": arguments }),
            )));
            assert_eq!(out["error"]["code"], INVALID_PARAMS, "{out}");
        }
        let out = reply(handle(&request(
            9,
            "tools/call",
            &json!({ "name": "other", "arguments": {} }),
        )));
        assert_eq!(out["error"]["code"], INVALID_PARAMS);
    }

    #[test]
    fn unknown_methods_error() {
        let out = reply(handle(&request(10, "resources/list", &json!({}))));
        assert_eq!(out["error"]["code"], METHOD_NOT_FOUND);
        assert_eq!(out["id"], 10);
    }

    #[test]
    fn tool_result_shapes() {
        let ok = tool_result(&json!(1), "hello");
        assert_eq!(ok["result"]["content"][0]["text"], "hello");
        assert_eq!(ok["result"]["isError"], false);
        let bad = tool_error(&json!("abc"), "nope");
        assert_eq!(bad["id"], "abc");
        assert_eq!(bad["result"]["isError"], true);
    }

    fn sample_web() -> ContextWeb {
        let root = "at://did:plc:a/app.bsky.feed.post/k1";
        let mut web = ContextWeb::new(root, "2026-01-01T00:00:00Z");
        let mut thread = Thread::new(root);
        thread.posts.insert(
            root.into(),
            Post::new(
                root,
                "c",
                Author {
                    did: "did:plc:a".into(),
                    handle: "alice.test".into(),
                    display_name: "Alice".into(),
                },
                "hello",
                "2026-01-01T00:00:00Z",
            ),
        );
        web.add_thread(thread);
        web
    }

    #[test]
    fn compose_headers() {
        let web = sample_web();
        let budget = CrawlOptions {
            max_nodes: 500,
            timeout: Duration::from_secs(20),
            ..CrawlOptions::default()
        };
        let complete = compose(
            &web,
            &Lens::Raw,
            Provenance::Crawled {
                stop_reason: StopReason::Complete,
                pending: 0,
            },
            &budget,
        );
        assert!(
            complete.starts_with(
                "# Bluesky context web: https://bsky.app/profile/alice.test/post/k1\n"
            )
        );
        assert!(complete.contains("1 posts, 1 threads, 0 edges. Lens: raw."));
        assert!(complete.contains("fully explored"));
        assert!(complete.contains("\"format_version\": 2"));

        let timed_out = compose(
            &web,
            &Lens::Raw,
            Provenance::Crawled {
                stop_reason: StopReason::Timeout,
                pending: 7,
            },
            &budget,
        );
        assert!(timed_out.contains("(20s per call); 7 threads unexplored"));
        let capped = compose(
            &web,
            &Lens::Raw,
            Provenance::Crawled {
                stop_reason: StopReason::MaxNodes,
                pending: 3,
            },
            &budget,
        );
        assert!(capped.contains("(500 posts per call); 3 threads unexplored"));
        let cached = compose(
            &web,
            &Lens::Raw,
            Provenance::Cached { age_secs: 42 },
            &budget,
        );
        assert!(cached.contains("cached web (42s old)"));
    }
}
