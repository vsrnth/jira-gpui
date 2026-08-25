//! Local persistence adapters.

mod event_semantics;
mod memory;
mod sqlite;

pub use memory::InMemoryStore;
pub use sqlite::{SqliteOpenError, SqliteStore};
