//! Incremental session index updates.
//!
//! Updates an existing SessionIndex when new events are appended to the JSONL file.

use serde::Deserialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::Path;

use crate::claude_code::FileEditType;

use super::builder::build_session_index;
use super::types::{EditMetadata, SessionIndex};

/// Result of an incremental update.
pub enum UpdateResult {
    /// Index was updated incrementally (fast path)
    Updated,
    /// Index was rebuilt from scratch (file was truncated or corrupted)
    Rebuilt,
    /// No update needed (file unchanged)
    Unchanged,
}

/// Update an existing session index incrementally.
///
/// If the file has grown (append-only), only parse new lines.
/// If the file has shrunk or been modified, rebuild entirely.
pub fn update_index_incremental(
    index: &mut SessionIndex,
    session_file: &Path,
    project_path: &str,
) -> Result<UpdateResult, String> {
    let metadata = fs::metadata(session_file)
        .map_err(|e| format!("Failed to read file metadata: {}", e))?;

    let current_size = metadata.len();
    let current_mtime = metadata.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);

    // Check if file is unchanged
    if current_size == index.file_size && current_mtime == index.last_modified {
        return Ok(UpdateResult::Unchanged);
    }

    // If file shrunk, rebuild entirely (compaction or corruption)
    if current_size < index.file_size {
        *index = build_session_index(session_file, project_path)?;
        return Ok(UpdateResult::Rebuilt);
    }

    // File grew - incrementally parse new content
    let mut file = File::open(session_file)
        .map_err(|e| format!("Failed to open session file: {}", e))?;

    // Seek to where we left off
    file.seek(SeekFrom::Start(index.file_size))
        .map_err(|e| format!("Failed to seek in file: {}", e))?;

    let reader = BufReader::new(file);
    let mut byte_offset = index.file_size;
    let start_sequence = index.line_offsets.len() as u32;

    // Track new file edits
    let mut new_file_operations: HashMap<String, FileEditType> = HashMap::new();
    let mut new_files_with_prior_content: HashSet<String> = HashSet::new();
    let mut new_file_timestamps: HashMap<String, String> = HashMap::new();

    for (rel_seq, line_result) in reader.lines().enumerate() {
        let line = match line_result {
            Ok(l) => l,
            Err(_) => continue,
        };

        let line_len = line.len() + 1; // +1 for newline
        let sequence = start_sequence + rel_seq as u32;

        // Record line offset
        index.line_offsets.push((byte_offset, line_len));

        // Parse the JSON entry
        if let Ok(entry) = serde_json::from_str::<JsonEntry>(&line) {
            // Extract UUID and parent UUID
            if let Some(ref uuid) = entry.uuid {
                index.uuid_to_line.insert(uuid.clone(), sequence);

                if let Some(ref parent) = entry.parent_uuid {
                    index.parent_map.insert(uuid.clone(), parent.clone());
                }
            }

            // Check if this is a human message boundary
            if is_human_message(&entry) {
                // Insert in sorted order
                match index.human_message_lines.binary_search(&sequence) {
                    Ok(_) => {} // Already exists
                    Err(pos) => index.human_message_lines.insert(pos, sequence),
                }
            }

            // Extract file edits from assistant messages
            if entry.entry_type.as_deref() == Some("assistant") {
                if let Some(ref message) = entry.message {
                    if let Some(ref content) = message.content {
                        if let Value::Array(items) = content {
                            for item in items {
                                process_tool_use_incremental(
                                    item,
                                    project_path,
                                    sequence,
                                    byte_offset,
                                    entry.uuid.as_deref(),
                                    entry.parent_uuid.as_deref(),
                                    entry.timestamp.as_deref(),
                                    index,
                                    &mut new_file_operations,
                                    &mut new_files_with_prior_content,
                                    &mut new_file_timestamps,
                                );
                            }
                        }
                    }
                }
            }
        }

        byte_offset += line_len as u64;
    }

    // Merge new file edits into existing
    merge_new_file_edits(
        index,
        new_file_operations,
        new_files_with_prior_content,
        new_file_timestamps,
    );

    // Update file state
    index.file_size = current_size;
    index.last_modified = current_mtime;

    Ok(UpdateResult::Updated)
}

/// Check if an entry is a human message.
fn is_human_message(entry: &JsonEntry) -> bool {
    if entry.entry_type.as_deref() != Some("user") {
        return false;
    }

    if entry.user_type.as_deref() != Some("external") {
        return false;
    }

    if let Some(ref message) = entry.message {
        if let Some(ref content) = message.content {
            if let Value::Array(items) = content {
                for item in items {
                    if item.get("type").and_then(|v| v.as_str()) == Some("tool_result") {
                        return false;
                    }
                }
            }
        }
    }

    if entry.is_compact_summary == Some(true) {
        return false;
    }

    if entry.is_meta == Some(true) {
        return false;
    }

    true
}

/// Process a tool_use for incremental updates.
fn process_tool_use_incremental(
    item: &Value,
    project_path: &str,
    sequence: u32,
    _byte_offset: u64,
    uuid: Option<&str>,
    _parent_uuid: Option<&str>,
    timestamp: Option<&str>,
    index: &mut SessionIndex,
    new_file_operations: &mut HashMap<String, FileEditType>,
    new_files_with_prior_content: &mut HashSet<String>,
    new_file_timestamps: &mut HashMap<String, String>,
) {
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

                if let Some(old_str) = input.get("old_string").and_then(|v| v.as_str()) {
                    if !old_str.is_empty() {
                        new_files_with_prior_content.insert(rel_path.clone());
                    }
                }

                new_file_operations.insert(rel_path.clone(), FileEditType::Modified);

                if let Some(ts) = timestamp {
                    new_file_timestamps.insert(rel_path.clone(), ts.to_string());
                }

                index.edit_metadata.insert(
                    sequence,
                    EditMetadata {
                        uuid: uuid.map(String::from),
                    },
                );

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

                // Check if this file already exists in the index
                let file_exists = index.file_to_edit_lines.contains_key(&rel_path);
                if !file_exists && !new_file_operations.contains_key(&rel_path) {
                    new_file_operations.insert(rel_path.clone(), FileEditType::Added);
                }

                if let Some(ts) = timestamp {
                    new_file_timestamps.insert(rel_path.clone(), ts.to_string());
                }

                index.edit_metadata.insert(
                    sequence,
                    EditMetadata {
                        uuid: uuid.map(String::from),
                    },
                );

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

/// Merge new file edits into the existing index.
fn merge_new_file_edits(
    index: &mut SessionIndex,
    new_file_operations: HashMap<String, FileEditType>,
    new_files_with_prior_content: HashSet<String>,
    new_file_timestamps: HashMap<String, String>,
) {
    use crate::claude_code::FileEdit;

    for (path, edit_type) in new_file_operations {
        // Find existing edit for this path
        if let Some(existing) = index.file_edits.iter_mut().find(|e| e.path == path) {
            // Update timestamp
            if let Some(ts) = new_file_timestamps.get(&path) {
                existing.last_edited_at = Some(ts.clone());
            }
            // If it was added before and now has prior content, it's modified
            if new_files_with_prior_content.contains(&path) {
                existing.edit_type = FileEditType::Modified;
            }
        } else {
            // New file edit
            let mut final_type = edit_type;
            if final_type == FileEditType::Modified && !new_files_with_prior_content.contains(&path)
            {
                final_type = FileEditType::Added;
            }

            index.file_edits.push(FileEdit {
                path: path.clone(),
                edit_type: final_type,
                last_edited_at: new_file_timestamps.get(&path).cloned(),
            });
        }
    }

    // Re-sort file edits
    index.file_edits.sort_by(|a, b| a.path.cmp(&b.path));
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
