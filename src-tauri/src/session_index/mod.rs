//! Session index module.
//!
//! Provides efficient indexing and querying of session JSONL files.
//!
//! ## Overview
//!
//! The session index is built once when a session is opened and updated
//! incrementally when the file changes. It provides:
//!
//! - O(1) UUID lookups
//! - O(1) file edit retrieval
//! - O(k) parent chain walking (for edit context)
//! - Pre-computed line offsets for fast pagination
//!
//! ## Usage
//!
//! ```ignore
//! // Build index for a session
//! let index = build_session_index(&session_file, &project_path)?;
//!
//! // Get status for frontend
//! let status = index.to_status();
//!
//! // Look up events by UUID
//! if let Some(line) = index.line_for_uuid("abc-123") {
//!     // Read event at that line
//! }
//!
//! // Get edit context
//! let context = get_edit_context(&index, &session_file, edit_line)?;
//! ```

mod builder;
mod queries;
mod types;
mod updater;

// Re-export public API
pub use builder::build_session_index;
pub use queries::{get_edit_context, EditContext};
pub use types::{IndexStatus, SessionIndex};
pub use updater::{update_index_incremental, UpdateResult};
