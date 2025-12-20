//! Query functions for the session index.
//!
//! Provides efficient lookups using the pre-built index.

use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::Path;

use crate::claude_code::{parse_session_event, SessionEvent};

use super::types::SessionIndex;

/// Context for a file edit - all events from the triggering user message to the edit.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditContext {
    /// Events in order from the human message to the edit
    pub events: Vec<SessionEvent>,
    /// Line number of the triggering human message
    pub trigger_line: u32,
    /// Line number of the edit itself
    pub edit_line: u32,
}

/// Get the context for a file edit.
///
/// Walks the parent chain backwards from the edit until it finds a human message boundary.
/// Returns all events in that range.
pub fn get_edit_context(
    index: &SessionIndex,
    session_file: &Path,
    edit_line: u32,
) -> Result<EditContext, String> {
    // Get the edit metadata
    let edit_meta = index
        .edit_metadata
        .get(&edit_line)
        .ok_or_else(|| format!("No edit metadata found for line {}", edit_line))?;

    // Walk backwards via parent chain to find the triggering human message
    let mut lines_in_context: Vec<u32> = vec![edit_line];
    let mut current_uuid = edit_meta.uuid.clone();

    // Walk parent chain
    while let Some(ref uuid) = current_uuid {
        if let Some(parent_uuid) = index.parent_of(uuid) {
            if let Some(parent_line) = index.line_for_uuid(parent_uuid) {
                // Check if this parent is a human message boundary
                if index.is_human_message(parent_line) {
                    lines_in_context.push(parent_line);
                    break;
                }
                lines_in_context.push(parent_line);
                current_uuid = Some(parent_uuid.clone());
            } else {
                break;
            }
        } else {
            break;
        }
    }

    // Reverse to get chronological order
    lines_in_context.reverse();

    // If we didn't find a human message, use the boundary finder
    let trigger_line = if lines_in_context.len() > 1
        && index.is_human_message(lines_in_context[0])
    {
        lines_in_context[0]
    } else {
        // Fallback: find the most recent human message before the edit
        index.find_human_boundary(edit_line).unwrap_or(0)
    };

    // Load the actual events
    let events = load_events_for_lines(index, session_file, &lines_in_context)?;

    Ok(EditContext {
        events,
        trigger_line,
        edit_line,
    })
}

/// Load SessionEvent objects for specific line numbers.
fn load_events_for_lines(
    index: &SessionIndex,
    session_file: &Path,
    lines: &[u32],
) -> Result<Vec<SessionEvent>, String> {
    let mut file =
        File::open(session_file).map_err(|e| format!("Failed to open session file: {}", e))?;

    let mut events = Vec::with_capacity(lines.len());

    for &line in lines {
        if let Some((offset, _length)) = index.line_offsets.get(line as usize) {
            if let Some(event) = read_event_at_offset(&mut file, *offset, line)? {
                events.push(event);
            }
        }
    }

    Ok(events)
}

/// Read a single event at a byte offset.
fn read_event_at_offset(
    file: &mut File,
    offset: u64,
    sequence: u32,
) -> Result<Option<SessionEvent>, String> {
    file.seek(SeekFrom::Start(offset))
        .map_err(|e| format!("Failed to seek: {}", e))?;

    let mut reader = BufReader::new(&*file);
    let mut line = String::new();

    reader
        .read_line(&mut line)
        .map_err(|e| format!("Failed to read line: {}", e))?;

    // Trim trailing newline
    if line.ends_with('\n') {
        line.pop();
    }
    if line.ends_with('\r') {
        line.pop();
    }

    Ok(parse_session_event(&line, sequence, offset))
}
