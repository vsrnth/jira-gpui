//! Local persistence adapters.
//!
//! The first implementation is intentionally in-memory. A SQLite adapter will
//! implement the same application ports once the synchronization flow is in
//! place.

mod memory;

pub use memory::InMemoryStore;
