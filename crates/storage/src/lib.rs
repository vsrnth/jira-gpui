//! Local persistence adapters.

mod event_semantics;
mod memory;
mod sqlite;

#[cfg(test)]
mod test_support;

pub use memory::InMemoryStore;
pub use sqlite::{SqliteOpenError, SqliteStore};
