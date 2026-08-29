//! Local storage of crawled webs as JSON files.
//!
//! Webs live in `$XDG_DATA_HOME/bsky-context/webs/` (default
//! `~/.local/share/bsky-context/webs/`), one file per web named by
//! [`web_id`]. The directory and format are shared with the original Python
//! tool, so webs it saved load unchanged.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use bsky_context_core::model::{ContextWeb, web_id};
use serde::Deserialize;

/// Returns the default web directory, honoring `XDG_DATA_HOME`.
pub fn default_data_dir() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::home_dir().map(|h| h.join(".local").join("share")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("bsky-context").join("webs")
}

/// Why a web could not be loaded.
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    /// No file matched the identifier.
    #[error("no web found for '{0}'")]
    NotFound(String),
    /// More than one file matched the identifier as a prefix.
    #[error("ambiguous ID '{id}', matches: {matches:?}")]
    Ambiguous {
        /// The identifier given.
        id: String,
        /// The web IDs it matched.
        matches: Vec<String>,
    },
    /// The file could not be read.
    #[error(transparent)]
    Io(#[from] io::Error),
    /// The file is not a valid web.
    #[error("invalid web file: {0}")]
    Parse(#[from] serde_json::Error),
}

/// Writes a web to `dir`, returning the path written.
///
/// # Errors
///
/// Returns an [`io::Error`] if the directory cannot be created or the file
/// cannot be written.
pub fn save_web(dir: &Path, web: &ContextWeb) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let path = dir.join(format!("{}.json", web_id(&web.root_uri)));
    fs::write(&path, web.to_json_pretty())?;
    Ok(path)
}

/// Loads a web by exact ID, or by a prefix that matches exactly one file.
///
/// # Errors
///
/// Returns [`LoadError::NotFound`] when nothing matches,
/// [`LoadError::Ambiguous`] when a prefix matches several webs, and the
/// underlying error when a file cannot be read or parsed.
pub fn load_web(dir: &Path, identifier: &str) -> Result<ContextWeb, LoadError> {
    let exact = dir.join(format!("{identifier}.json"));
    if exact.is_file() {
        return read_web(&exact);
    }
    let mut matches: Vec<String> = web_files(dir)?
        .into_iter()
        .filter_map(|p| stem(&p))
        .filter(|id| id.starts_with(identifier))
        .collect();
    matches.sort();
    match matches.as_slice() {
        [] => Err(LoadError::NotFound(identifier.to_owned())),
        [only] => read_web(&dir.join(format!("{only}.json"))),
        _ => Err(LoadError::Ambiguous {
            id: identifier.to_owned(),
            matches,
        }),
    }
}

fn read_web(path: &Path) -> Result<ContextWeb, LoadError> {
    let json = fs::read_to_string(path)?;
    Ok(ContextWeb::from_json(&json)?)
}

/// What `list` shows for each stored web, read from the file's `meta`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebSummary {
    /// The web ID (file stem).
    pub id: String,
    /// Root post URI.
    pub root_uri: String,
    /// When it was crawled.
    pub crawled_at: String,
    /// Post count.
    pub nodes: usize,
    /// Edge count.
    pub edges: usize,
    /// Thread count.
    pub threads: usize,
}

#[derive(Deserialize)]
struct MetaOnly {
    #[serde(default)]
    meta: Meta,
}

#[derive(Deserialize, Default)]
struct Meta {
    #[serde(default)]
    root_uri: Option<String>,
    #[serde(default)]
    crawled_at: Option<String>,
    #[serde(default)]
    node_count: usize,
    #[serde(default)]
    edge_count: usize,
    #[serde(default)]
    thread_count: usize,
}

/// Lists stored webs sorted by ID, reading only each file's `meta`.
///
/// # Errors
///
/// Returns an [`io::Error`] if the directory or a file cannot be read; a
/// missing directory yields an empty list.
pub fn list_webs(dir: &Path) -> io::Result<Vec<WebSummary>> {
    let mut result = Vec::new();
    for path in web_files(dir)? {
        let Some(id) = stem(&path) else {
            continue;
        };
        let json = fs::read_to_string(&path)?;
        let Meta {
            root_uri,
            crawled_at,
            node_count,
            edge_count,
            thread_count,
        } = serde_json::from_str::<MetaOnly>(&json)
            .map(|m| m.meta)
            .unwrap_or_default();
        result.push(WebSummary {
            id,
            root_uri: root_uri.unwrap_or_else(|| "?".to_owned()),
            crawled_at: crawled_at.unwrap_or_else(|| "?".to_owned()),
            nodes: node_count,
            edges: edge_count,
            threads: thread_count,
        });
    }
    Ok(result)
}

fn web_files(dir: &Path) -> io::Result<Vec<PathBuf>> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut files: Vec<PathBuf> = fs::read_dir(dir)?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    files.sort();
    Ok(files)
}

fn stem(path: &Path) -> Option<String> {
    path.file_stem().map(|s| s.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use bsky_context_core::model::{Author, Post, Thread};

    use super::*;

    fn make_web(root_uri: &str) -> ContextWeb {
        let mut web = ContextWeb::new(root_uri, "2026-01-01T00:00:00Z");
        let mut thread = Thread::new(root_uri);
        thread.posts.insert(
            root_uri.into(),
            Post::new(
                root_uri,
                "cid1",
                Author {
                    did: "did:plc:test".into(),
                    handle: "test.bsky.social".into(),
                    display_name: String::new(),
                },
                "Test post",
                "2026-01-01T00:00:00Z",
            ),
        );
        web.add_thread(thread);
        web
    }

    const DEFAULT_ROOT: &str = "at://did:plc:test/app.bsky.feed.post/abc123";

    #[test]
    fn save_then_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let web = make_web(DEFAULT_ROOT);
        let path = save_web(dir.path(), &web).unwrap();
        let loaded = load_web(dir.path(), &stem(&path).unwrap()).unwrap();
        assert_eq!(loaded.node_count(), 1);
        assert_eq!(loaded.thread_count(), 1);
        assert_eq!(loaded.root_uri, web.root_uri);
        assert_eq!(loaded, web);
    }

    #[test]
    fn load_by_prefix() {
        let dir = tempfile::tempdir().unwrap();
        save_web(dir.path(), &make_web(DEFAULT_ROOT)).unwrap();
        let loaded = load_web(dir.path(), "abc123").unwrap();
        assert_eq!(loaded.node_count(), 1);
    }

    #[test]
    fn load_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let err = load_web(dir.path(), "nonexistent").unwrap_err();
        assert!(matches_not_found(&err));
        let missing = dir.path().join("does-not-exist");
        assert!(matches_not_found(&load_web(&missing, "x").unwrap_err()));
    }

    fn matches_not_found(err: &LoadError) -> bool {
        match err {
            LoadError::NotFound(_) => true,
            LoadError::Ambiguous { .. } | LoadError::Io(_) | LoadError::Parse(_) => false,
        }
    }

    #[test]
    fn load_ambiguous() {
        let dir = tempfile::tempdir().unwrap();
        save_web(
            dir.path(),
            &make_web("at://did:plc:a/app.bsky.feed.post/abc1"),
        )
        .unwrap();
        save_web(
            dir.path(),
            &make_web("at://did:plc:b/app.bsky.feed.post/abc2"),
        )
        .unwrap();
        let err = load_web(dir.path(), "abc").unwrap_err();
        let LoadError::Ambiguous { id, matches } = err else {
            panic!("expected Ambiguous, got {err:?}");
        };
        assert_eq!(id, "abc");
        assert_eq!(matches.len(), 2);
        assert!(err_text_mentions(&matches, "abc1-"));
    }

    fn err_text_mentions(matches: &[String], prefix: &str) -> bool {
        matches.iter().any(|m| m.starts_with(prefix))
    }

    #[test]
    fn load_parse_error() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("bad-000000.json"), "{not json").unwrap();
        let err = load_web(dir.path(), "bad-000000").unwrap_err();
        let LoadError::Parse(_) = err else {
            panic!("expected Parse, got {err:?}");
        };
    }

    #[test]
    fn list_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(list_webs(dir.path()).unwrap().is_empty());
        assert!(list_webs(&dir.path().join("missing")).unwrap().is_empty());
    }

    #[test]
    fn list_saved() {
        let dir = tempfile::tempdir().unwrap();
        save_web(dir.path(), &make_web(DEFAULT_ROOT)).unwrap();
        let webs = list_webs(dir.path()).unwrap();
        assert_eq!(webs.len(), 1);
        let WebSummary {
            id,
            root_uri,
            crawled_at,
            nodes,
            edges,
            threads,
        } = &webs[0];
        assert!(id.starts_with("abc123-"));
        assert_eq!(root_uri, DEFAULT_ROOT);
        assert_eq!(crawled_at, "2026-01-01T00:00:00Z");
        assert_eq!((*nodes, *edges, *threads), (1, 0, 1));
    }

    #[test]
    fn list_multiple_sorted() {
        let dir = tempfile::tempdir().unwrap();
        save_web(
            dir.path(),
            &make_web("at://did:plc:a/app.bsky.feed.post/zzz"),
        )
        .unwrap();
        save_web(
            dir.path(),
            &make_web("at://did:plc:b/app.bsky.feed.post/aaa"),
        )
        .unwrap();
        let ids: Vec<String> = list_webs(dir.path())
            .unwrap()
            .into_iter()
            .map(|w| w.id)
            .collect();
        assert_eq!(ids.len(), 2);
        assert!(ids[0].starts_with("aaa-"));
        assert!(ids[1].starts_with("zzz-"));
    }

    #[test]
    fn list_tolerates_missing_meta() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("old-000000.json"), "{}").unwrap();
        let webs = list_webs(dir.path()).unwrap();
        assert_eq!(webs[0].root_uri, "?");
        assert_eq!(webs[0].nodes, 0);
    }
}
