//! # AI Enablement core
//!
//! The portable behaviour of the in-app AI agent, without UI: the context-sync protocol, the system prompt, tool
//! contracts and argument validation, permissions, skills, agents and memory; the session (the tool loop, built-in
//! tools, saved chats: [`session`]); the capability folder ([`capabilities`]); the provider adapters and their
//! transport; and the host's rules ([`policy`]). A UI crate (the egui drawer) and the
//! host application build on it.
//!
//! The JS runtime of the skill (`assets/ai-agent/`, runtime 1.7.0) is the reference implementation. This crate is
//! "the same agent" when it passes the shared conformance vectors in `spec/conformance/` (see
//! `tests/conformance.rs`) and follows `spec/behaviour.md`. Where the two disagree, the JS runtime is right.
//!
//! Strings are measured as JS measures them (UTF-16 code units) and JSON keeps its key order ([`json::Value`]), so
//! prompts, hashes and saved chats are the same byte for byte in every runtime.

pub mod adapters;
pub mod agents;
pub mod blocks;
pub mod capabilities;
pub mod chat;
pub mod client;
pub mod context;
pub mod conversation;
pub mod files;
pub mod frontmatter;
pub mod hash;
#[cfg(feature = "http")]
pub mod http;
pub mod json;
pub mod memory;
pub mod permissions;
pub mod policy;
pub mod prompt;
pub mod providers;
pub mod reasoning;
pub mod schema;
pub mod session;
pub mod settings;
pub mod skills;
pub mod sse;
pub mod text;
pub mod time;
pub mod tools;
pub mod transport;

/// The version of this crate (the Rust runtime). `scripts/detect.mjs` reads it from a vendored copy.
pub const VERSION: &str = "0.1.0";

/// The JS runtime release whose behaviour (and conformance vectors) this version follows.
pub const REFERENCE_RUNTIME: &str = "assets/ai-agent 1.7.0";

pub use json::{Map, Value};
