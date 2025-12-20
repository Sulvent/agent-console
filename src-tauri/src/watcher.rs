//! Session file watcher for real-time edit updates.
//!
//! Watches Claude Code session JSONL files and emits Tauri events when changes occur.
//! Also manages session indices for fast lookups.

use notify_debouncer_mini::{new_debouncer, notify::RecursiveMode, DebouncedEventKind};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Emitter};

use crate::session_index::{
    build_session_index, update_index_incremental, IndexStatus, SessionIndex, UpdateResult,
};

/// Event payload sent to the frontend when a session file changes.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionChangedPayload {
    pub project_path: String,
    pub session_id: String,
}

/// Event payload sent to the frontend when a sub-agent file changes.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubagentChangedPayload {
    pub project_path: String,
    pub agent_id: String,
}

/// Event payload sent to the frontend when the session index is ready.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexReadyPayload {
    pub project_path: String,
    pub session_id: String,
    pub status: IndexStatus,
}

/// Global state for managing file watchers and session indices.
pub struct WatcherState {
    /// Map of "project_path:session_id" -> watcher handle (for cleanup)
    watchers: Mutex<HashMap<String, WatcherHandle>>,
    /// Map of "project_path:session_id" -> session index (for fast lookups)
    /// Wrapped in Arc so it can be shared with background indexing threads
    indices: Arc<Mutex<HashMap<String, SessionIndex>>>,
}

struct WatcherHandle {
    // The debouncer is kept alive by holding this reference
    _debouncer: notify_debouncer_mini::Debouncer<notify::RecommendedWatcher>,
}

impl WatcherState {
    pub fn new() -> Self {
        Self {
            watchers: Mutex::new(HashMap::new()),
            indices: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Get a clone of the indices Arc for sharing with background threads.
    fn indices_arc(&self) -> Arc<Mutex<HashMap<String, SessionIndex>>> {
        Arc::clone(&self.indices)
    }

    /// Get the index for a session, if it exists.
    pub fn get_index(&self, project_path: &str, session_id: &str) -> Option<SessionIndex> {
        let key = format!("{}:{}", project_path, session_id);
        let indices = self.indices.lock().ok()?;
        indices.get(&key).cloned()
    }

    /// Get the index status for a session.
    pub fn get_index_status(&self, project_path: &str, session_id: &str) -> IndexStatus {
        let key = format!("{}:{}", project_path, session_id);
        let indices = match self.indices.lock() {
            Ok(i) => i,
            Err(_) => return IndexStatus::error("Failed to lock indices"),
        };

        match indices.get(&key) {
            Some(index) => index.to_status(),
            None => IndexStatus::building(),
        }
    }
}

/// Get the session file path for watching.
fn get_session_file_path(project_path: &str, session_id: &str) -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let encoded_name = project_path.replace('/', "-");
    let session_file = home
        .join(".claude")
        .join("projects")
        .join(&encoded_name)
        .join(format!("{}.jsonl", session_id));

    if session_file.exists() {
        Some(session_file)
    } else {
        None
    }
}

/// Start watching a session file for changes.
/// Spawns a background thread to build the session index, emitting "index-ready" when done.
pub fn watch_session(
    app_handle: AppHandle,
    state: &WatcherState,
    project_path: String,
    session_id: String,
) -> Result<(), String> {
    let key = format!("{}:{}", project_path, session_id);

    // Check if already watching
    {
        let watchers = state.watchers.lock().map_err(|e| e.to_string())?;
        if watchers.contains_key(&key) {
            return Ok(()); // Already watching
        }
    }

    let session_file = get_session_file_path(&project_path, &session_id)
        .ok_or_else(|| format!("Session file not found for {}", session_id))?;

    // Clone data for the file watcher closure
    let watcher_app_handle = app_handle.clone();
    let watcher_project_path = project_path.clone();
    let watcher_session_id = session_id.clone();
    let watcher_session_file = session_file.clone();
    let watcher_indices = state.indices_arc();
    let watcher_key = key.clone();

    // Create debounced watcher with 500ms debounce
    let mut debouncer = new_debouncer(
        Duration::from_millis(500),
        move |result: Result<Vec<notify_debouncer_mini::DebouncedEvent>, notify::Error>| {
            if let Ok(events) = result {
                for event in events {
                    if event.kind == DebouncedEventKind::Any {
                        // Update the index incrementally
                        if let Ok(mut indices) = watcher_indices.lock() {
                            if let Some(index) = indices.get_mut(&watcher_key) {
                                match update_index_incremental(
                                    index,
                                    &watcher_session_file,
                                    &watcher_project_path,
                                ) {
                                    Ok(UpdateResult::Updated) => {
                                        println!(
                                            "[session_index] Incremental update: now {} events",
                                            index.total_events()
                                        );
                                    }
                                    Ok(UpdateResult::Rebuilt) => {
                                        println!(
                                            "[session_index] Index rebuilt: {} events",
                                            index.total_events()
                                        );
                                    }
                                    Ok(UpdateResult::Unchanged) => {
                                        // No logging for unchanged
                                    }
                                    Err(e) => {
                                        eprintln!("[session_index] Incremental update failed: {}", e);
                                    }
                                }
                            }
                        }

                        // Emit event to frontend
                        let _ = watcher_app_handle.emit(
                            "session-changed",
                            SessionChangedPayload {
                                project_path: watcher_project_path.clone(),
                                session_id: watcher_session_id.clone(),
                            },
                        );
                        break; // Only emit once per batch
                    }
                }
            }
        },
    )
    .map_err(|e| format!("Failed to create watcher: {}", e))?;

    // Watch the session file
    debouncer
        .watcher()
        .watch(&session_file, RecursiveMode::NonRecursive)
        .map_err(|e| format!("Failed to watch file: {}", e))?;

    // Store the watcher handle immediately (so cleanup works)
    {
        let mut watchers = state.watchers.lock().map_err(|e| e.to_string())?;
        watchers.insert(
            key.clone(),
            WatcherHandle {
                _debouncer: debouncer,
            },
        );
    }

    // Clone data for the background indexing thread
    let indices = state.indices_arc();
    let index_app_handle = app_handle;
    let index_project_path = project_path;
    let index_session_id = session_id;
    let index_session_file = session_file;
    let index_key = key;

    // Spawn background thread to build the index
    std::thread::spawn(move || {
        let status = match build_session_index(&index_session_file, &index_project_path) {
            Ok(index) => {
                // Log index stats for verification
                println!(
                    "[session_index] Built index for {}: {} events, {} file edits, {} files edited",
                    index_session_id,
                    index.total_events(),
                    index.file_edits.len(),
                    index.file_to_edit_lines.len()
                );

                let status = index.to_status();

                // Store the index
                if let Ok(mut indices) = indices.lock() {
                    indices.insert(index_key, index);
                }

                status
            }
            Err(err) => {
                eprintln!("[session_index] Failed to build index: {}", err);
                IndexStatus::error(err)
            }
        };

        // Emit index-ready event to frontend
        let _ = index_app_handle.emit(
            "index-ready",
            IndexReadyPayload {
                project_path: index_project_path,
                session_id: index_session_id,
                status,
            },
        );
    });

    Ok(())
}

/// Stop watching a session file and clean up its index.
pub fn unwatch_session(
    state: &WatcherState,
    project_path: &str,
    session_id: &str,
) -> Result<(), String> {
    let key = format!("{}:{}", project_path, session_id);

    // Remove the watcher
    {
        let mut watchers = state.watchers.lock().map_err(|e| e.to_string())?;
        watchers.remove(&key);
    }

    // Remove the index
    {
        let mut indices = state.indices.lock().map_err(|e| e.to_string())?;
        indices.remove(&key);
    }

    Ok(())
}

/// Get the sub-agent file path for watching.
fn get_subagent_file_path(project_path: &str, agent_id: &str) -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    let encoded_name = project_path.replace('/', "-");
    let agent_file = home
        .join(".claude")
        .join("projects")
        .join(&encoded_name)
        .join(format!("agent-{}.jsonl", agent_id));

    if agent_file.exists() {
        Some(agent_file)
    } else {
        None
    }
}

/// Start watching a sub-agent file for changes.
pub fn watch_subagent(
    app_handle: AppHandle,
    state: &WatcherState,
    project_path: String,
    agent_id: String,
) -> Result<(), String> {
    let key = format!("{}:agent:{}", project_path, agent_id);

    // Check if already watching
    {
        let watchers = state.watchers.lock().map_err(|e| e.to_string())?;
        if watchers.contains_key(&key) {
            return Ok(()); // Already watching
        }
    }

    let agent_file = get_subagent_file_path(&project_path, &agent_id)
        .ok_or_else(|| format!("Sub-agent file not found for {}", agent_id))?;

    let project_path_clone = project_path.clone();
    let agent_id_clone = agent_id.clone();

    // Create debounced watcher with 500ms debounce
    let mut debouncer = new_debouncer(
        Duration::from_millis(500),
        move |result: Result<Vec<notify_debouncer_mini::DebouncedEvent>, notify::Error>| {
            if let Ok(events) = result {
                for event in events {
                    if event.kind == DebouncedEventKind::Any {
                        // Emit event to frontend
                        let _ = app_handle.emit(
                            "subagent-changed",
                            SubagentChangedPayload {
                                project_path: project_path_clone.clone(),
                                agent_id: agent_id_clone.clone(),
                            },
                        );
                        break; // Only emit once per batch
                    }
                }
            }
        },
    )
    .map_err(|e| format!("Failed to create watcher: {}", e))?;

    // Watch the agent file
    debouncer
        .watcher()
        .watch(&agent_file, RecursiveMode::NonRecursive)
        .map_err(|e| format!("Failed to watch file: {}", e))?;

    // Store the watcher handle
    {
        let mut watchers = state.watchers.lock().map_err(|e| e.to_string())?;
        watchers.insert(
            key,
            WatcherHandle {
                _debouncer: debouncer,
            },
        );
    }

    Ok(())
}

/// Stop watching a sub-agent file.
pub fn unwatch_subagent(
    state: &WatcherState,
    project_path: &str,
    agent_id: &str,
) -> Result<(), String> {
    let key = format!("{}:agent:{}", project_path, agent_id);

    let mut watchers = state.watchers.lock().map_err(|e| e.to_string())?;
    watchers.remove(&key);

    Ok(())
}

/// Event payload sent to the frontend when telemetry files change.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TelemetryChangedPayload {
    pub project_path: String,
}

/// Get the telemetry directory path for a project.
fn get_telemetry_dir_path(project_path: &str) -> PathBuf {
    PathBuf::from(project_path)
        .join(".cupcake")
        .join("telemetry")
}

/// Start watching a project's telemetry directory for changes.
pub fn watch_telemetry(
    app_handle: AppHandle,
    state: &WatcherState,
    project_path: String,
) -> Result<(), String> {
    let key = format!("{}:telemetry", project_path);

    // Check if already watching
    {
        let watchers = state.watchers.lock().map_err(|e| e.to_string())?;
        if watchers.contains_key(&key) {
            return Ok(()); // Already watching
        }
    }

    let telemetry_dir = get_telemetry_dir_path(&project_path);

    // Create the directory if it doesn't exist (so we can watch it)
    if !telemetry_dir.exists() {
        std::fs::create_dir_all(&telemetry_dir)
            .map_err(|e| format!("Failed to create telemetry dir: {}", e))?;
    }

    let project_path_clone = project_path.clone();

    // Create debounced watcher with 300ms debounce
    let mut debouncer = new_debouncer(
        Duration::from_millis(300),
        move |result: Result<Vec<notify_debouncer_mini::DebouncedEvent>, notify::Error>| {
            if let Ok(events) = result {
                for event in events {
                    if event.kind == DebouncedEventKind::Any {
                        // Only emit for JSON files
                        if event
                            .path
                            .extension()
                            .map(|e| e == "json")
                            .unwrap_or(false)
                        {
                            let _ = app_handle.emit(
                                "telemetry-changed",
                                TelemetryChangedPayload {
                                    project_path: project_path_clone.clone(),
                                },
                            );
                            break; // Only emit once per batch
                        }
                    }
                }
            }
        },
    )
    .map_err(|e| format!("Failed to create watcher: {}", e))?;

    // Watch the telemetry directory
    debouncer
        .watcher()
        .watch(&telemetry_dir, RecursiveMode::NonRecursive)
        .map_err(|e| format!("Failed to watch telemetry dir: {}", e))?;

    // Store the watcher handle
    {
        let mut watchers = state.watchers.lock().map_err(|e| e.to_string())?;
        watchers.insert(
            key,
            WatcherHandle {
                _debouncer: debouncer,
            },
        );
    }

    Ok(())
}

/// Stop watching a project's telemetry directory.
pub fn unwatch_telemetry(state: &WatcherState, project_path: &str) -> Result<(), String> {
    let key = format!("{}:telemetry", project_path);

    let mut watchers = state.watchers.lock().map_err(|e| e.to_string())?;
    watchers.remove(&key);

    Ok(())
}
