//! Session index types.
//!
//! This module defines the core data structures for the session index,
//! which provides fast lookups into session JSONL files.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::SystemTime;

use crate::claude_code::FileEdit;

/// Index for a single session's JSONL file.
///
/// Built once when a session is opened, updated incrementally on file changes.
/// Provides O(1) lookups for UUIDs, file edits, and parent chain walking.
#[derive(Debug, Clone)]
pub struct SessionIndex {
    // === File State (for incremental updates) ===
    /// Size of file when index was last built/updated
    pub file_size: u64,
    /// Modification time when index was last built/updated
    pub last_modified: SystemTime,

    // === Line Index (for pagination) ===
    /// (byte_offset, line_length) for each line in the file
    pub line_offsets: Vec<(u64, usize)>,

    // === UUID Lookups (for chain walking) ===
    /// UUID → sequence number (line index)
    pub uuid_to_line: HashMap<String, u32>,

    // === Parent Chain (for edit context) ===
    /// UUID → parentUuid (for walking the conversation chain)
    pub parent_map: HashMap<String, String>,

    // === Human Message Boundaries (for edit context) ===
    /// Sequence numbers of "me" messages (actual human input, not tool results)
    /// Used to find where a conversation segment starts
    pub human_message_lines: Vec<u32>,

    // === Pre-computed File Edits ===
    /// All file edits extracted from this session
    pub file_edits: Vec<FileEdit>,
    /// file_path → sequence numbers of edits to that file
    pub file_to_edit_lines: HashMap<String, Vec<u32>>,

    // === Edit Metadata (for context feature) ===
    /// Sequence number → (byte_offset, messageId) for edits
    /// Allows looking up the message context for any edit
    pub edit_metadata: HashMap<u32, EditMetadata>,
}

/// Metadata for a single file edit event.
#[derive(Debug, Clone)]
pub struct EditMetadata {
    /// UUID of this event (for parent chain walking)
    pub uuid: Option<String>,
}

/// Status of the session index, returned to frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexStatus {
    /// Whether the index is ready to use
    pub ready: bool,
    /// Total number of events in the session
    pub total_events: u32,
    /// Number of file edits found
    pub file_edits_count: u32,
    /// Number of unique files edited
    pub files_edited_count: u32,
    /// Error message if indexing failed
    pub error: Option<String>,
}

impl SessionIndex {
    /// Create an empty index (used before building).
    pub fn empty() -> Self {
        Self {
            file_size: 0,
            last_modified: SystemTime::UNIX_EPOCH,
            line_offsets: Vec::new(),
            uuid_to_line: HashMap::new(),
            parent_map: HashMap::new(),
            human_message_lines: Vec::new(),
            file_edits: Vec::new(),
            file_to_edit_lines: HashMap::new(),
            edit_metadata: HashMap::new(),
        }
    }

    /// Get total number of events (lines) in the session.
    pub fn total_events(&self) -> u32 {
        self.line_offsets.len() as u32
    }

    /// Look up the line number for a UUID.
    pub fn line_for_uuid(&self, uuid: &str) -> Option<u32> {
        self.uuid_to_line.get(uuid).copied()
    }

    /// Get the parent UUID for a given UUID.
    pub fn parent_of(&self, uuid: &str) -> Option<&String> {
        self.parent_map.get(uuid)
    }

    /// Check if a line is a human message boundary.
    pub fn is_human_message(&self, line: u32) -> bool {
        self.human_message_lines.binary_search(&line).is_ok()
    }

    /// Find the most recent human message at or before a given line.
    pub fn find_human_boundary(&self, line: u32) -> Option<u32> {
        match self.human_message_lines.binary_search(&line) {
            Ok(idx) => Some(self.human_message_lines[idx]),
            Err(0) => None, // No human message before this line
            Err(idx) => Some(self.human_message_lines[idx - 1]),
        }
    }

    /// Create IndexStatus for frontend.
    pub fn to_status(&self) -> IndexStatus {
        IndexStatus {
            ready: true,
            total_events: self.total_events(),
            file_edits_count: self.file_edits.len() as u32,
            files_edited_count: self.file_to_edit_lines.len() as u32,
            error: None,
        }
    }
}

impl IndexStatus {
    /// Create a "building" status.
    pub fn building() -> Self {
        Self {
            ready: false,
            total_events: 0,
            file_edits_count: 0,
            files_edited_count: 0,
            error: None,
        }
    }

    /// Create an error status.
    pub fn error(msg: impl Into<String>) -> Self {
        Self {
            ready: false,
            total_events: 0,
            file_edits_count: 0,
            files_edited_count: 0,
            error: Some(msg.into()),
        }
    }
}
