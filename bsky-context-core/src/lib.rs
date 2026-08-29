//! Core domain logic for Bluesky Context Webs.
//!
//! A Context Web is the full conversation graph around a Bluesky post: the
//! reply tree, plus every post that quotes any post in it, plus *their*
//! reply trees, recursively. This crate holds the data model, the crawler,
//! and the lenses that render a web as text for humans or language models.
//!
//! The crate is pure: no I/O and no platform assumptions. HTTP and time
//! come in through the [`Fetch`] and [`Clock`] traits so the same crawler
//! runs in a browser, a Cloudflare Worker, and a native CLI.
//!
//! [`Fetch`]: crate::api::Fetch
//! [`Clock`]: crate::api::Clock

pub mod api;
pub mod crawler;
pub mod lens;
pub mod model;
pub mod uri;
