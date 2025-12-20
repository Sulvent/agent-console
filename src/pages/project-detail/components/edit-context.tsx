import { useState, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Panel,
  PanelGroup,
  PanelResizeHandle,
} from "react-resizable-panels";
import {
  IconChevronDown,
  IconChevronRight,
  IconLoader2,
  IconGripVertical,
  IconX,
  IconCopy,
  IconCheck,
} from "@tabler/icons-react";
import { cn } from "@/lib/utils";
import { JsonViewer } from "./json-viewer";
import { getEventDisplayLabel, formatTimestamp } from "../utils";
import type { EditContext, SessionEvent } from "@/lib/types";

interface EditContextViewProps {
  projectPath: string;
  sessionId: string;
  filePath: string;
  editIndex: number;
}

/**
 * Shows the event chain for a file edit - from the human message to the edit.
 * Clicking an event opens a detail panel with the raw JSON.
 */
export function EditContextView({
  projectPath,
  sessionId,
  filePath,
  editIndex,
}: EditContextViewProps) {
  const [isExpanded, setIsExpanded] = useState(false);
  const [context, setContext] = useState<EditContext | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Selected event for detail view
  const [selectedEvent, setSelectedEvent] = useState<SessionEvent | null>(null);
  const [rawJson, setRawJson] = useState<string | null>(null);
  const [rawJsonLoading, setRawJsonLoading] = useState(false);
  const [jsonCopied, setJsonCopied] = useState(false);

  // Load context when expanded
  useEffect(() => {
    if (!isExpanded || context) return;

    async function loadContext() {
      setLoading(true);
      setError(null);
      try {
        const result = await invoke<EditContext>("get_file_edit_context", {
          projectPath,
          sessionId,
          filePath,
          editIndex,
        });
        setContext(result);
      } catch (err) {
        console.error("Failed to load edit context:", err);
        setError(err instanceof Error ? err.message : String(err));
      } finally {
        setLoading(false);
      }
    }

    loadContext();
  }, [isExpanded, context, projectPath, sessionId, filePath, editIndex]);

  // Load raw JSON when event is selected
  useEffect(() => {
    if (!selectedEvent) {
      setRawJson(null);
      return;
    }

    async function loadRawJson() {
      setRawJsonLoading(true);
      try {
        const json = await invoke<string | null>("get_event_raw_json", {
          projectPath,
          sessionId,
          byteOffset: selectedEvent.byteOffset,
        });
        setRawJson(json);
      } catch (err) {
        console.error("Failed to load raw JSON:", err);
        setRawJson(null);
      } finally {
        setRawJsonLoading(false);
      }
    }

    loadRawJson();
  }, [selectedEvent, projectPath, sessionId]);

  return (
    <div className="border-t border-border">
      {/* Toggle header */}
      <button
        onClick={() => setIsExpanded(!isExpanded)}
        className="w-full flex items-center gap-2 px-3 py-1.5 text-xs text-muted-foreground hover:text-foreground hover:bg-muted/50 transition-colors"
      >
        {isExpanded ? (
          <IconChevronDown className="size-3" />
        ) : (
          <IconChevronRight className="size-3" />
        )}
        <span>What led to this edit</span>
        {loading && <IconLoader2 className="size-3 animate-spin ml-auto" />}
      </button>

      {/* Context content */}
      {isExpanded && (
        <div className="border-t border-border">
          {loading ? (
            <div className="flex items-center gap-2 text-xs text-muted-foreground py-4 px-3">
              <IconLoader2 className="size-3 animate-spin" />
              Loading context...
            </div>
          ) : error ? (
            <div className="text-xs text-destructive py-4 px-3">{error}</div>
          ) : context && context.events.length > 0 ? (
            <PanelGroup direction="horizontal" className="min-h-[150px]">
              {/* Event list - reversed so edit is first, human message is last */}
              <Panel defaultSize={selectedEvent ? 50 : 100} minSize={30}>
                <div className="h-full overflow-auto bg-muted/30">
                  {[...context.events].reverse().map((event, index, arr) => (
                    <button
                      key={event.sequence}
                      onClick={() => setSelectedEvent(event)}
                      className={cn(
                        "w-full text-left px-3 py-1.5 text-xs border-b border-border/50 hover:bg-muted/50 transition-colors",
                        selectedEvent?.sequence === event.sequence && "bg-accent",
                        index === 0 && "text-amber-600 dark:text-amber-400 font-medium",
                        index === arr.length - 1 && "text-blue-600 dark:text-blue-400"
                      )}
                    >
                      <div className="flex items-center justify-between gap-2">
                        <span className="font-medium truncate">
                          {getEventDisplayLabel(event)}
                        </span>
                        <span className="text-muted-foreground shrink-0">
                          {event.timestamp ? formatTimestamp(event.timestamp) : `#${event.sequence}`}
                        </span>
                      </div>
                      <p className="text-muted-foreground truncate mt-0.5">
                        {event.preview.length > 80
                          ? event.preview.slice(0, 80) + "..."
                          : event.preview}
                      </p>
                    </button>
                  ))}
                </div>
              </Panel>

              {/* Detail panel */}
              {selectedEvent && (
                <>
                  <PanelResizeHandle className="w-1.5 bg-border hover:bg-primary/50 transition-colors flex items-center justify-center group">
                    <IconGripVertical className="size-3 text-muted-foreground opacity-0 group-hover:opacity-100 transition-opacity" />
                  </PanelResizeHandle>
                  <Panel defaultSize={50} minSize={20}>
                    <div className="h-full flex flex-col border-l border-border bg-background">
                      {/* Header */}
                      <div className="shrink-0 px-3 py-2 border-b border-border flex items-center justify-between">
                        <div className="flex items-center gap-2 text-xs">
                          <span className="font-medium">{getEventDisplayLabel(selectedEvent)}</span>
                          <span className="text-muted-foreground">#{selectedEvent.sequence}</span>
                        </div>
                        <div className="flex items-center gap-1">
                          <button
                            onClick={() => {
                              if (rawJson) {
                                navigator.clipboard.writeText(rawJson);
                                setJsonCopied(true);
                                setTimeout(() => setJsonCopied(false), 2000);
                              }
                            }}
                            className="p-1 rounded hover:bg-muted transition-colors"
                            title="Copy JSON"
                          >
                            {jsonCopied ? (
                              <IconCheck className="size-3.5 text-green-500" />
                            ) : (
                              <IconCopy className="size-3.5 text-muted-foreground" />
                            )}
                          </button>
                          <button
                            onClick={() => setSelectedEvent(null)}
                            className="p-1 rounded hover:bg-muted transition-colors"
                            title="Close"
                          >
                            <IconX className="size-3.5 text-muted-foreground" />
                          </button>
                        </div>
                      </div>
                      {/* JSON content */}
                      <div className="flex-1 overflow-auto p-2 text-[11px]">
                        {rawJsonLoading ? (
                          <div className="flex items-center gap-2 text-muted-foreground">
                            <IconLoader2 className="size-3 animate-spin" />
                            Loading...
                          </div>
                        ) : rawJson ? (
                          <JsonViewer data={JSON.parse(rawJson)} defaultExpanded isRoot />
                        ) : (
                          <div className="text-muted-foreground">
                            No data available
                          </div>
                        )}
                      </div>
                    </div>
                  </Panel>
                </>
              )}
            </PanelGroup>
          ) : (
            <div className="text-xs text-muted-foreground py-4 px-3">
              No context available
            </div>
          )}
        </div>
      )}
    </div>
  );
}
