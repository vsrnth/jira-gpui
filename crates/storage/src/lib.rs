//! Local persistence adapters.

mod memory;
mod sqlite;

pub use memory::InMemoryStore;
pub use sqlite::{SqliteOpenError, SqliteStore};
