export interface Thread {
  id: string;
  provider: string;
  model: string | null;
  title: string | null;
  cwd: string | null;
  pinned: boolean;
  worktree_path: string | null;
  branch: string | null;
  updated_at: number;
}

export interface ThreadEvent {
  seq: number;
  kind: string;
  payload: any;
  created_at: number;
}

export interface Provider {
  id: string;
  display_name: string;
  installed: boolean;
  auth: { state: string; account?: string | null };
}

export interface ApprovalRequest {
  id: string;
  tool: string;
  input: unknown;
}

export interface FileStatus {
  index: string;
  worktree: string;
  path: string;
}

/** Evento normalizado do núcleo: `{ type, data }`. */
export type AgentEvent =
  | { type: "session_started"; data: { session_id: string; model: string | null } }
  | { type: "text_delta" | "thinking_delta" | "error"; data: string }
  | { type: "tool_use"; data: { id: string; name: string; input: unknown } }
  | { type: "tool_result"; data: { id: string; output: string; is_error: boolean } }
  | { type: "approval_requested"; data: ApprovalRequest }
  | { type: "turn_complete"; data: { text: string | null; is_error: boolean } };

export interface AgentEventPayload {
  threadId: string;
  event: AgentEvent;
}

export interface TurnFinished {
  threadId: string;
  isError: boolean;
  text: string;
}
