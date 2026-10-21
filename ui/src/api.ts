import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { AgentEventPayload, ApprovalRequest, FileStatus, Provider, Thread, ThreadEvent, TurnFinished } from "./types.ts";

/** Fora do Tauri (preview no navegador) a UI usa dados de mentira. */
export const inTauri = "__TAURI_INTERNALS__" in window;

type Listener<T> = (payload: T) => void;

export interface Api {
  listProviders(): Promise<Provider[]>;
  listSessions(): Promise<Thread[]>;
  createSession(provider: string, cwd: string | null): Promise<Thread>;
  deleteSession(threadId: string): Promise<void>;
  getMessages(threadId: string): Promise<ThreadEvent[]>;
  sendMessage(threadId: string, prompt: string, mode: string): Promise<void>;
  stopSession(threadId: string): Promise<void>;
  respondApproval(threadId: string, id: string, approved: boolean): Promise<void>;
  gitStatus(cwd: string): Promise<FileStatus[]>;
  gitDiff(cwd: string, staged: boolean): Promise<string>;
  onAgentEvent(fn: Listener<AgentEventPayload>): void;
  onTurnFinished(fn: Listener<TurnFinished>): void;
  onSessionsChanged(fn: Listener<void>): void;
}

const real: Api = {
  listProviders: () => invoke("list_providers"),
  listSessions: () => invoke("list_sessions"),
  createSession: (provider, cwd) => invoke("create_session", { provider, cwd, title: null }),
  deleteSession: (threadId) => invoke("delete_session", { threadId }),
  getMessages: (threadId) => invoke("get_messages", { threadId }),
  sendMessage: (threadId, prompt, mode) => invoke("send_message", { threadId, prompt, mode }),
  stopSession: (threadId) => invoke("stop_session", { threadId }),
  respondApproval: (threadId, id, approved) => invoke("respond_approval", { threadId, id, approved }),
  gitStatus: (cwd) => invoke("git_status", { cwd }),
  gitDiff: (cwd, staged) => invoke("git_diff", { cwd, staged }),
  onAgentEvent: (fn) => void listen<AgentEventPayload>("agent-event", (e) => fn(e.payload)),
  onTurnFinished: (fn) => void listen<TurnFinished>("turn-finished", (e) => fn(e.payload)),
  onSessionsChanged: (fn) => void listen("sessions-changed", () => fn()),
};

function createMock(): Api {
  const listeners = { agent: [] as Listener<AgentEventPayload>[], done: [] as Listener<TurnFinished>[], sessions: [] as Listener<void>[] };
  const threads: Thread[] = [];
  const events = new Map<string, ThreadEvent[]>();
  const pending = new Map<string, (ok: boolean) => void>();
  let seq = 0;
  const push = (id: string, kind: string, payload: unknown) => events.get(id)!.push({ seq: ++seq, kind, payload, created_at: Date.now() });
  const emit = (threadId: string, event: AgentEventPayload["event"]) => listeners.agent.forEach((f) => f({ threadId, event }));
  const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

  return {
    listProviders: async () => [
      { id: "claude", display_name: "Claude Code", installed: true, auth: { state: "authenticated", account: "demo" } },
      { id: "codex", display_name: "Codex", installed: false, auth: { state: "unavailable" } },
    ],
    listSessions: async () => [...threads],
    createSession: async (provider, cwd) => {
      const t: Thread = { id: crypto.randomUUID(), provider, model: null, title: null, cwd, pinned: false, worktree_path: null, branch: null, updated_at: Date.now() };
      threads.unshift(t);
      events.set(t.id, []);
      listeners.sessions.forEach((f) => f());
      return t;
    },
    deleteSession: async (id) => {
      threads.splice(threads.findIndex((t) => t.id === id), 1);
      listeners.sessions.forEach((f) => f());
    },
    getMessages: async (id) => [...(events.get(id) ?? [])],
    sendMessage: async (id, prompt) => {
      push(id, "user", { text: prompt });
      void (async () => {
        await sleep(300);
        emit(id, { type: "tool_use", data: { id: "t1", name: "Bash", input: { command: "ls" } } });
        push(id, "tool_use", { id: "t1", name: "Bash", input: { command: "ls" } });
        emit(id, { type: "approval_requested", data: { id: "a1", tool: "Bash", input: { command: "rm -rf build" } } });
        const ok = await new Promise<boolean>((r) => pending.set(`${id}:a1`, r));
        push(id, "approval", { id: "a1", tool: "Bash", input: { command: "rm -rf build" }, approved: ok });
        const text = ok ? "Feito. **Removi** `build`." : "Ok, não executei.";
        for (const word of text.split(" ")) {
          emit(id, { type: "text_delta", data: word + " " });
          await sleep(60);
        }
        push(id, "changes", { files: [{ path: "src/main.rs", added: 12, deleted: 3, patch: "diff --git a/src/main.rs b/src/main.rs\n-old\n+new", truncated: false }] });
        push(id, "assistant", { text, is_error: false });
        listeners.done.forEach((f) => f({ threadId: id, isError: false, text }));
      })();
    },
    stopSession: async () => {},
    respondApproval: async (id, reqId, ok) => void pending.get(`${id}:${reqId}`)?.(ok),
    gitStatus: async () => [{ index: " ", worktree: "M", path: "src/main.rs" }],
    gitDiff: async () => "diff --git a/src/main.rs b/src/main.rs\n--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1 +1 @@\n-old\n+new\n",
    onAgentEvent: (fn) => void listeners.agent.push(fn),
    onTurnFinished: (fn) => void listeners.done.push(fn),
    onSessionsChanged: (fn) => void listeners.sessions.push(fn),
  };
}

export const api: Api = inTauri ? real : createMock();
export type { ApprovalRequest };
