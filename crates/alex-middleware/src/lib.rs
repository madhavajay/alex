//! Runtime-neutral middleware objects, declarative matching, and policy decisions.
//!
//! This crate intentionally has no HTTP framework, async runtime, account store, or
//! provider client dependencies. The proxy owns dispatch and passes safe snapshots
//! through this API.

mod builtins;
mod dto;
mod engine;
mod guard;
mod headers;
mod rule;
mod validate;

pub use builtins::*;
pub use dto::*;
pub use engine::*;
pub use guard::*;
pub use headers::*;
pub use rule::*;
pub use validate::*;

/// The only middleware schema version understood by this release.
pub const API_VERSION_V1: u16 = 1;
