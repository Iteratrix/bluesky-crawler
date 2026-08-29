//! Command-line adapter over the core crate.
//!
//! Thin by design: argument handling and file I/O live here; crawling and
//! rendering live in core. Doubles as a scriptable harness for the same
//! code the web app and the worker run.

mod client;
mod storage;

use core::fmt::Write as _;
use core::time::Duration;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Context;
use bsky_context_core::api::PUBLIC_APPVIEW;
use bsky_context_core::crawler::{CrawlOptions, CrawlResult, Progress, StopReason, crawl};
use bsky_context_core::lens::{LensKind, LensParams, render};
use bsky_context_core::uri::PostRef;
use clap::{Args, Parser, Subcommand};

use crate::client::{HttpFetch, TokioClock};
use crate::storage::{LoadError, default_data_dir, list_webs, load_web, save_web};

#[derive(Parser)]
#[command(
    name = "bsky-context",
    version,
    about = "Crawl and explore Bluesky conversation graphs"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Crawl a Bluesky conversation graph starting from a post
    ///
    /// If a previous crawl exists for this post it is loaded and updated
    /// with new posts. Use --fresh to discard the stored version.
    Fetch(FetchArgs),
    /// Render a stored context web through a lens
    Show(ShowArgs),
    /// List all stored context webs
    List,
}

#[derive(Args)]
struct FetchArgs {
    /// An AT URI or a bsky.app post URL
    post_url: String,
    /// Maximum posts to crawl
    #[arg(long, default_value_t = 2000)]
    max_nodes: usize,
    /// Maximum quote-hop distance from the start post
    #[arg(long)]
    max_depth: Option<usize>,
    /// Maximum wall-clock seconds for the crawl
    #[arg(long, default_value_t = 300.0)]
    timeout: f64,
    /// Discard the stored version and crawl from scratch
    #[arg(long)]
    fresh: bool,
    /// Maximum concurrent API requests
    #[arg(short, long, default_value_t = 2)]
    concurrency: usize,
    /// Show detailed logging (rate limits, retries, errors)
    #[arg(short, long)]
    verbose: bool,
}

#[derive(Args)]
struct ShowArgs {
    /// The identifier printed by `fetch`, or a unique prefix of it
    web_id: String,
    /// View to render
    #[arg(short, long, default_value = "tree")]
    lens: LensKind,
    /// Quote-chain hops (neighborhood lens)
    #[arg(long)]
    hops: Option<usize>,
    /// Target post URI (neighborhood lens)
    #[arg(long)]
    uri: Option<String>,
    /// Show posts at or after this ISO timestamp (timeline lens)
    #[arg(long)]
    after: Option<String>,
    /// Show posts before this ISO timestamp (timeline lens)
    #[arg(long)]
    before: Option<String>,
    /// Text search query (search lens)
    #[arg(short, long)]
    query: Option<String>,
    /// Filter by author handle (search lens)
    #[arg(long)]
    author: Option<String>,
    /// Number of results (threads/highlights lens)
    #[arg(short = 'n', long)]
    top: Option<usize>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Fetch(args) => run_fetch(args),
        Command::Show(args) => run_show(&args),
        Command::List => run_list(),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("Error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn data_dir() -> PathBuf {
    default_data_dir()
}

fn emit(text: &str) -> anyhow::Result<()> {
    let mut stdout = std::io::stdout().lock();
    let written = stdout
        .write_all(text.as_bytes())
        .and_then(|()| stdout.flush());
    match written {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::BrokenPipe => std::process::exit(0),
        Err(err) => Err(err.into()),
    }
}

fn run_fetch(args: FetchArgs) -> anyhow::Result<()> {
    let FetchArgs {
        post_url,
        max_nodes,
        max_depth,
        timeout,
        fresh,
        concurrency,
        verbose,
    } = args;
    let level = if verbose { "debug" } else { "warn" };
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(level))
        .format(|buf, record| writeln!(buf, "  {} {}", record.level(), record.args()))
        .init();

    let post_ref = PostRef::parse(&post_url)?;
    let dir = data_dir();
    let existing = if fresh {
        None
    } else {
        match load_web(&dir, &post_ref.rkey) {
            Ok(web) => {
                eprintln!("  Updating existing web ({} posts)...", web.node_count());
                Some(web)
            }
            Err(LoadError::NotFound(_)) => None,
            Err(err) => return Err(err.into()),
        }
    };

    let options = CrawlOptions {
        max_nodes,
        max_depth,
        timeout: Duration::from_secs_f64(timeout),
        concurrency,
    };
    let runtime = tokio::runtime::Runtime::new().context("starting async runtime")?;
    let CrawlResult {
        web,
        stop_reason,
        pending,
    } = runtime.block_on(async {
        let fetch = HttpFetch::new(PUBLIC_APPVIEW, Duration::from_secs(30))?;
        let clock = TokioClock::new();
        let mut on_progress = |progress: Progress| {
            let Progress {
                node_count,
                edge_count,
                thread_count,
            } = progress;
            let elapsed = clock_elapsed_secs(&clock);
            eprint!(
                "\r  Crawling... {node_count} posts, {thread_count} threads, {edge_count} edges ({elapsed}s)"
            );
        };
        let result = crawl(
            &fetch,
            &clock,
            &post_ref.at_uri(),
            &options,
            existing,
            &mut on_progress,
        )
        .await;
        eprintln!();
        eprintln!(
            "  Done in {:.1}s: {} posts, {} threads, {} edges",
            clock.elapsed_secs_f64(),
            result.web.node_count(),
            result.web.thread_count(),
            result.web.edge_count()
        );
        anyhow::Ok(result)
    })?;
    match stop_reason {
        StopReason::Complete => {}
        StopReason::MaxNodes => {
            eprintln!("  Stopped at --max-nodes {max_nodes}; {pending} threads unexplored");
        }
        StopReason::Timeout => {
            eprintln!("  Stopped at --timeout {timeout}s; {pending} threads unexplored");
        }
    }

    let path = save_web(&dir, &web).context("saving web")?;
    let id = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    eprintln!("  Saved: {id}");
    emit(&format!("{id}\n"))
}

fn clock_elapsed_secs(clock: &TokioClock) -> u64 {
    bsky_context_core::api::Clock::elapsed(clock).as_secs()
}

impl TokioClock {
    fn elapsed_secs_f64(&self) -> f64 {
        bsky_context_core::api::Clock::elapsed(self).as_secs_f64()
    }
}

fn run_show(args: &ShowArgs) -> anyhow::Result<()> {
    let ShowArgs {
        web_id,
        lens,
        hops,
        uri,
        after,
        before,
        query,
        author,
        top,
    } = args;
    let web = load_web(&data_dir(), web_id)?;
    let params = LensParams {
        top: *top,
        hops: *hops,
        uri: uri.clone(),
        after: after.clone(),
        before: before.clone(),
        query: query.clone(),
        author: author.clone(),
    };
    let mut text = render(&web, &lens.with_params(&params));
    text.push('\n');
    emit(&text)
}

fn run_list() -> anyhow::Result<()> {
    let webs = list_webs(&data_dir()).context("reading web directory")?;
    if webs.is_empty() {
        eprintln!("No stored context webs.");
        return Ok(());
    }
    let mut text = String::new();
    for summary in webs {
        let storage::WebSummary {
            id,
            root_uri,
            crawled_at,
            nodes,
            edges: _,
            threads,
        } = summary;
        let _ = writeln!(
            text,
            "{id}  {nodes} posts  {threads} threads  {crawled_at}\n  {root_uri}"
        );
    }
    emit(&text)
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn parses_show_flags() {
        let cli = Cli::parse_from([
            "bsky-context",
            "show",
            "abc",
            "-l",
            "search",
            "-q",
            "topic",
            "--author",
            "alice",
            "-n",
            "3",
        ]);
        let Command::Show(args) = cli.command else {
            panic!("expected show");
        };
        assert_eq!(args.lens, LensKind::Search);
        assert_eq!(args.query.as_deref(), Some("topic"));
        assert_eq!(args.top, Some(3));
    }

    #[test]
    fn rejects_unknown_lens() {
        let result = Cli::try_parse_from(["bsky-context", "show", "abc", "-l", "nope"]);
        assert!(result.is_err());
    }

    #[test]
    fn fetch_defaults_match_python() {
        let cli = Cli::parse_from([
            "bsky-context",
            "fetch",
            "at://did:plc:a/app.bsky.feed.post/1",
        ]);
        let Command::Fetch(args) = cli.command else {
            panic!("expected fetch");
        };
        assert_eq!(args.max_nodes, 2000);
        assert_eq!(args.concurrency, 2);
        assert!(!args.fresh);
        assert!((args.timeout - 300.0).abs() < f64::EPSILON);
    }
}
