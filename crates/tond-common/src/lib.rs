// tond-common — shared library crate
//
// This crate is the single source of truth for every type that crosses a
// crate boundary. Both tond-server and tond-cli depend on it so they never
// have to duplicate struct definitions or re-implement helpers.
//
// Two sub-modules:
//   errors  — the unified TondError enum used across the workspace
//   models  — all domain structs, DTOs, and request/response types
//
// Everything is re-exported flat from the crate root so consumers can write
// `use tond_common::App` instead of `use tond_common::models::App`.

pub mod errors;
pub mod models;

pub use errors::*;
pub use models::*;
