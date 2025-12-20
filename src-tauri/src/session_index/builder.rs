//! Session index builder.
//!
//! Builds a SessionIndex from a session JSONL file in a single pass.

use serde::Deserialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::Path;

use crate::claude_code::{FileEdit, FileEditType};

use super::types::{EditMetadata, SessionIndex};

/// Build a complete session index from a JSONL file.
///
/// This function reads the entire file once, extracting:
/// - Line offsets for pagination
/// - UUID mappings for lookups
/// - Parent chain for context walking
/// - Human message boundaries
/// - File edits
pub fn build_session_index(session_file: &Path, project_path: &str) -> Result<SessionIndex, String> {
    let metadata = fs::metadata(session_file)
        .map_err(|e| format!("Failed to read file metadata: {}", e))?;

    let file = File::open(session_file)
        .map_err(|e| format!("Failed to open session file: {}", e))?;

    let reader = BufReader::new(file);
    let mut index = SessionIndex::empty();

    // Track file state
    index.file_size = metadata.len();
    index.last_modified = metadata
        .modified()
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);

    // Track file edits (need to determine added vs modified)
    let mut file_operations: HashMap<String, FileEditType> = HashMap::new();
    let mut files_with_prior_content: HashSet<String> = HashSet::new();
    let mut file_timestamps: HashMap<String, String> = HashMap::new();

    let mut byte_offset: u64 = 0;

    for (sequence, line_result) in reader.lines().enumerate() {
        let line = match line_result {
            Ok(l) => l,
            Err(_) => continue,
        };

        let line_len = line.len() + 1; // +1 for newline
        let seq = sequence as u32;

        // Record line offset
        index.line_offsets.push((byte_offset, line_len));

        // Parse the JSON entry
        if let Ok(entry) = serde_json::from_str::<JsonEntry>(&line) {
            // Extract UUID and parent UUID
            if let Some(ref uuid) = entry.uuid {
                index.uuid_to_line.insert(uuid.clone(), seq);

                if let Some(ref parent) = entry.parent_uuid {
                    index.parent_map.insert(uuid.clone(), parent.clone());
                }
            }

            // Check if this is a human message boundary
            if is_human_message(&entry) {
                index.human_message_lines.push(seq);
            }

            // Extract file edits from assistant messages
            if entry.entry_type.as_deref() == Some("assistant") {
                if let Some(ref message) = entry.message {
                    if let Some(ref content) = message.content {
                        if let Value::Array(items) = content {
                            for item in items {
                                process_tool_use(
                                    item,
                                    project_path,
                                    seq,
                                    byte_offset,
                                    entry.uuid.as_deref(),
                                    entry.parent_uuid.as_deref(),
                                    entry.timestamp.as_deref(),
                                    &mut index,
                                    &mut file_operations,
                                    &mut files_with_prior_content,
                                    &mut file_timestamps,
                                );
                            }
                        }
                    }
                }
            }
        }

        byte_offset += line_len as u64;
    }

    // Build final file edits list
    finalize_file_edits(
        &mut index,
        file_operations,
        files_with_prior_content,
        file_timestamps,
    );

    // Sort human message lines for binary search
    index.human_message_lines.sort();

    Ok(index)
}

/// Check if an entry is a human message (actual user input, not tool results).
fn is_human_message(entry: &JsonEntry) -> bool {
    // Must be a user message
    if entry.entry_type.as_deref() != Some("user") {
        return false;
    }

    // Must have userType: "external"
    if entry.user_type.as_deref() != Some("external") {
        return false;
    }

    // Must NOT be a tool result
    if let Some(ref message) = entry.message {
        if let Some(ref content) = message.content {
            // Tool results have content as array with type: "tool_result"
            if let Value::Array(items) = content {
                for item in items {
                    if item.get("type").and_then(|v| v.as_str()) == Some("tool_result") {
                        return false;
                    }
                }
            }
        }
    }

    // Must NOT be a compact summary
    if entry.is_compact_summary == Some(true) {
        return false;
    }

    // Must NOT be meta
    if entry.is_meta == Some(true) {
        return false;
    }

    true
}

/// Process a potential tool_use entry for file edits.
fn process_tool_use(
    item: &Value,
    project_path: &str,
    sequence: u32,
    _byte_offset: u64,
    uuid: Option<&str>,
    _parent_uuid: Option<&str>,
    timestamp: Option<&str>,
    index: &mut SessionIndex,
    file_operations: &mut HashMap<String, FileEditType>,
    files_with_prior_content: &mut HashSet<String>,
    file_timestamps: &mut HashMap<String, String>,
) {
    // Check if this is a tool_use
    if item.get("type").and_then(|v| v.as_str()) != Some("tool_use") {
        return;
    }

    let tool_name = match item.get("name").and_then(|v| v.as_str()) {
        Some(n) => n,
        None => return,
    };

    let input = match item.get("input") {
        Some(i) => i,
        None => return,
    };

    match tool_name {
        "Edit" => {
            if let Some(file_path) = input.get("file_path").and_then(|v| v.as_str()) {
                let rel_path = make_relative_path(file_path, project_path);

                // Check if this edit has old_string content (indicates existing file)
                if let Some(old_str) = input.get("old_string").and_then(|v| v.as_str()) {
                    if !old_str.is_empty() {
                        files_with_prior_content.insert(rel_path.clone());
                    }
                }

                // Mark as modified
                file_operations.insert(rel_path.clone(), FileEditType::Modified);

                // Track timestamp
                if let Some(ts) = timestamp {
                    file_timestamps.insert(rel_path.clone(), ts.to_string());
                }

                // Record edit metadata
                index.edit_metadata.insert(
                    sequence,
                    EditMetadata {
                        uuid: uuid.map(String::from),
                    },
                );

                // Track line for this file
                index
                    .file_to_edit_lines
                    .entry(rel_path)
                    .or_default()
                    .push(sequence);
            }
        }
        "Write" => {
            if let Some(file_path) = input.get("file_path").and_then(|v| v.as_str()) {
                let rel_path = make_relative_path(file_path, project_path);

                // Write to a file that wasn't previously edited = added
                if !file_operations.contains_key(&rel_path) {
                    file_operations.insert(rel_path.clone(), FileEditType::Added);
                }

                // Track timestamp
                if let Some(ts) = timestamp {
                    file_timestamps.insert(rel_path.clone(), ts.to_string());
                }

                // Record edit metadata
                index.edit_metadata.insert(
                    sequence,
                    EditMetadata {
                        uuid: uuid.map(String::from),
                    },
                );

                // Track line for this file
                index
                    .file_to_edit_lines
                    .entry(rel_path)
                    .or_default()
                    .push(sequence);
            }
        }
        _ => {}
    }
}

/// Finalize file edits list, determining added vs modified.
fn finalize_file_edits(
    index: &mut SessionIndex,
    file_operations: HashMap<String, FileEditType>,
    files_with_prior_content: HashSet<String>,
    file_timestamps: HashMap<String, String>,
) {
    let mut edits: Vec<FileEdit> = file_operations
        .into_iter()
        .map(|(path, mut edit_type)| {
            // If a file was written but never had prior content, it's "added"
            if edit_type == FileEditType::Modified && !files_with_prior_content.contains(&path) {
                edit_type = FileEditType::Added;
            }
            let last_edited_at = file_timestamps.get(&path).cloned();
            FileEdit {
                path,
                edit_type,
                last_edited_at,
            }
        })
        .collect();

    // Sort by path for consistent display
    edits.sort_by(|a, b| a.path.cmp(&b.path));
    index.file_edits = edits;
}

/// Convert an absolute file path to a relative path from the project root.
fn make_relative_path(file_path: &str, project_path: &str) -> String {
    let project = project_path.trim_end_matches('/');
    if file_path.starts_with(project) {
        file_path[project.len()..]
            .trim_start_matches('/')
            .to_string()
    } else {
        file_path.to_string()
    }
}

// === JSON Parsing Structures ===

#[derive(Deserialize)]
struct JsonEntry {
    #[serde(rename = "type")]
    entry_type: Option<String>,
    uuid: Option<String>,
    #[serde(rename = "parentUuid")]
    parent_uuid: Option<String>,
    #[serde(rename = "userType")]
    user_type: Option<String>,
    #[serde(rename = "isCompactSummary")]
    is_compact_summary: Option<bool>,
    #[serde(rename = "isMeta")]
    is_meta: Option<bool>,
    message: Option<JsonMessage>,
    timestamp: Option<String>,
}

#[derive(Deserialize)]
struct JsonMessage {
    content: Option<Value>,
}
