//! SQLite persistence for CowBot.
//!
//! The store owns the database pool and exposes repository methods grouped by
//! domain area (identity, collaboration, agents/templates, tasks/runs/approvals,
//! and audit). Migrations are embedded into the binary and applied with
//! [`Store::migrate`].

mod error;
mod migrate;
mod repos;
mod store;

pub use store::Store;
