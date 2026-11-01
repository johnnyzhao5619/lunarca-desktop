export type TaskStatusKind =
  | "idle"
  | "queued"
  | "processing"
  | "needs_action"
  | "completed"
  | "failed"
  | "degraded"
  | "cancelled";

export type TaskStatusTone = "ai" | "attention" | "success" | "error" | "warning" | "muted";

export function taskStatusTone(kind: TaskStatusKind): TaskStatusTone {
  switch (kind) {
    case "queued":
    case "processing":
      return "ai";
    case "needs_action":
      return "attention";
    case "completed":
      return "success";
    case "failed":
      return "error";
    case "degraded":
      return "warning";
    case "idle":
    case "cancelled":
      return "muted";
  }
}

export function TaskStatus({ kind, label }: { kind: TaskStatusKind; label: string }) {
  return (
    <span className="luna-task-status" data-tone={taskStatusTone(kind)}>
      {label}
    </span>
  );
}
