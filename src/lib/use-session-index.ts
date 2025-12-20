import { useState, useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { IndexStatus } from "./types";

export type IndexState = "idle" | "indexing" | "ready" | "error";

interface UseSessionIndexResult {
  /** Current indexing state */
  state: IndexState;
  /** Whether the index is currently being built */
  isIndexing: boolean;
  /** Whether the index is ready for use */
  isReady: boolean;
  /** Index status with counts (available when ready) */
  status: IndexStatus | null;
  /** Error message if indexing failed */
  error: string | null;
}

interface SessionChangedPayload {
  projectPath: string;
  sessionId: string;
}

interface IndexReadyPayload {
  projectPath: string;
  sessionId: string;
  status: IndexStatus;
}

/**
 * Hook to manage session indexing.
 *
 * When a session is selected, this hook:
 * 1. Calls watch_session() which returns immediately and spawns background indexing
 * 2. Listens for "index-ready" event when indexing completes
 * 3. Listens for "session-changed" events for file change notifications
 * 4. Cleans up the watcher on unmount or session change
 *
 * @param projectPath - The project path
 * @param sessionId - The selected session ID (null if none)
 * @param onSessionChanged - Callback when session file changes (for refreshing data)
 */
export function useSessionIndex(
  projectPath: string,
  sessionId: string | null,
  onSessionChanged?: () => void
): UseSessionIndexResult {
  const [state, setState] = useState<IndexState>("idle");
  const [status, setStatus] = useState<IndexStatus | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Use ref for callback to avoid re-subscribing when callback changes
  const onSessionChangedRef = useRef(onSessionChanged);
  onSessionChangedRef.current = onSessionChanged;

  useEffect(() => {
    if (!sessionId) {
      setState("idle");
      setStatus(null);
      setError(null);
      return;
    }

    let cancelled = false;
    let unlistenIndexReady: (() => void) | null = null;
    let unlistenSessionChanged: (() => void) | null = null;

    async function setupSession() {
      setState("indexing");
      setError(null);
      setStatus(null);

      try {
        // Listen for index-ready event BEFORE calling watch_session
        // to ensure we don't miss the event
        unlistenIndexReady = await listen<IndexReadyPayload>(
          "index-ready",
          (event) => {
            if (cancelled) return;
            if (
              event.payload.projectPath === projectPath &&
              event.payload.sessionId === sessionId
            ) {
              const indexStatus = event.payload.status;
              if (indexStatus.error) {
                setState("error");
                setError(indexStatus.error);
                setStatus(null);
              } else {
                setState("ready");
                setStatus(indexStatus);
                setError(null);
              }
            }
          }
        );

        // Listen for session-changed events
        unlistenSessionChanged = await listen<SessionChangedPayload>(
          "session-changed",
          (event) => {
            if (
              event.payload.projectPath === projectPath &&
              event.payload.sessionId === sessionId
            ) {
              onSessionChangedRef.current?.();
            }
          }
        );

        // Start watching (returns immediately, indexing happens in background)
        await invoke("watch_session", {
          projectPath,
          sessionId,
        });
      } catch (err) {
        if (cancelled) return;
        setState("error");
        setError(err instanceof Error ? err.message : String(err));
        setStatus(null);
      }
    }

    setupSession();

    return () => {
      cancelled = true;
      if (unlistenIndexReady) {
        unlistenIndexReady();
      }
      if (unlistenSessionChanged) {
        unlistenSessionChanged();
      }
      // Clean up the watcher
      invoke("unwatch_session", {
        projectPath,
        sessionId,
      }).catch((err) => {
        console.error("Failed to stop session watcher:", err);
      });
    };
  }, [projectPath, sessionId]); // onSessionChanged removed - using ref instead

  return {
    state,
    isIndexing: state === "indexing",
    isReady: state === "ready",
    status,
    error,
  };
}
