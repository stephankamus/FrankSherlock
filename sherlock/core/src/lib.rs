//! Read core of the Frank Sherlock index.
//!
//! The desktop app owns the pipeline — scanning, classification, faces, publishing —
//! and the web server only reads. Everything both sides must agree on lives here, so
//! that "the web shows what the desktop found" is a structural property rather than a
//! promise: one schema, one query parser, one set of row shapes.
//!
//! The split is deliberately partial for now: `error`, the query parser and the types
//! it returns. `db` and the read models follow, and moving them is a file move plus a
//! re-export in the desktop crate — the modules themselves are already stateless
//! (`fn(db_path: &Path)` everywhere), so nothing here ever learned about Tauri.

pub mod error;
pub mod exif;
pub mod models;
pub mod paths;
pub mod query_parser;
pub mod similarity;
