import { api } from "./api.ts";
import "./style.css";
import { escapeHtml, renderMarkdown, summarizeInput } from "./text.ts";
import type { AgentEventPayload, ApprovalRequest, Provider, Thread, ThreadEvent } from "./types.ts";

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

const state = {
  providers: [] as Provider[],
  sessions: [] as Thread[],
  current: null as Thread | null,
  events: [] as ThreadEvent[],
  /** A última busca encheu a página: pode haver eventos mais antigos no banco. */
  hasMore: false,
  /** Turnos em andamento, por sessão; guarda o que chegou em streaming. */
  live: new Map<string, { text: string; steps: string[]; approvals: ApprovalRequest[] }>(),
};

function toast(message: string) {
  const el = $("toast");
  el.textContent = message;
  el.classList.add("show");
  setTimeout(() => el.classList.remove("show"), 4000);
}

const guard = <A extends unknown[]>(fn: (...args: A) => Promise<void>) => (...args: A) =>
  fn(...args).catch((e) => toast(String(e)));

function el(tag: string, className?: string, text?: string): HTMLElement {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text !== undefined) node.textContent = text;
  return node;
}

// ---------- tarefas ----------

const PAGE = 300;
const DAY = 24 * 60 * 60 * 1000;

function renderSessions() {
  const box = $("sessions");
  box.replaceChildren();
  const groups: [string, Thread[]][] = [
    ["Ativas", state.sessions.filter((t) => Date.now() - t.updated_at < DAY)],
    ["Inativas", state.sessions.filter((t) => Date.now() - t.updated_at >= DAY)],
  ];
  for (const [label, list] of groups) {
    if (!list.length) continue;
    box.append(el("div", "group-label", label));
    for (const t of list) {
      const b = el("button", "session") as HTMLButtonElement;
      b.type = "button";
      b.setAttribute("aria-current", String(t.id === state.current?.id));
      b.append(t.title ?? "(sem título)");
      b.append(el("small", undefined, `${t.provider}${t.branch ? " · " + t.branch : ""}${state.live.has(t.id) ? " · trabalhando…" : ""}`));
      b.onclick = guard(() => select(t));
      box.append(b);
    }
  }
}

async function refreshSessions() {
  state.sessions = await api.listSessions();
  if (state.current) state.current = state.sessions.find((t) => t.id === state.current!.id) ?? null;
  renderSessions();
  renderContext();
}

/** Traz os eventos mais recentes e mantém os antigos já carregados. */
async function reload(threadId: string) {
  const latest = await api.getMessages(threadId);
  const older = latest.length ? state.events.filter((e) => e.seq > 0 && e.seq < latest[0].seq) : [];
  state.events = [...older, ...latest];
  if (!older.length) state.hasMore = latest.length >= PAGE;
}

async function loadOlder() {
  const t = state.current;
  const first = state.events.find((e) => e.seq > 0);
  if (!t || !first) return;
  const page = await api.getMessages(t.id, first.seq);
  state.events = [...page, ...state.events];
  state.hasMore = page.length >= PAGE;
  renderThread();
}

async function select(thread: Thread) {
  state.current = thread;
  state.events = [];
  await reload(thread.id);
  renderAll();
  void guard(refreshGit)();
}

// ---------- conversa ----------

function renderContext() {
  const t = state.current;
  $("context-text").textContent = t ? `${t.cwd ?? "sem pasta"} · ${t.worktree_path ? "Worktree" : "Local"}${t.branch ? " · " + t.branch : ""} · ${t.provider}` : "Selecione ou crie uma tarefa";
  $("worktree-form").hidden = !t || !t.cwd || !!t.worktree_path;
}

/** Agrupa ferramentas e aprovações de cada turno num bloco recolhido; texto, erros e mudanças ficam fora. */
function renderThread() {
  const box = $("thread");
  const stick = box.scrollTop + box.clientHeight >= box.scrollHeight - 40;
  box.replaceChildren();
  if (state.hasMore) {
    const more = el("button", undefined, "Carregar mensagens anteriores") as HTMLButtonElement;
    more.type = "button";
    more.onclick = guard(loadOlder);
    box.append(more);
  }
  let steps: HTMLElement[] = [];
  const flush = () => {
    if (!steps.length) return;
    const d = el("details", "work") as HTMLDetailsElement;
    d.append(el("summary", undefined, `Trabalhou em ${steps.length} ${steps.length === 1 ? "ação" : "ações"}`), ...steps);
    box.append(d);
    steps = [];
  };
  for (const e of state.events) {
    const p = e.payload;
    if (e.kind === "tool_use") steps.push(el("div", "step", `${p.name} ${summarizeInput(p.input)}`));
    else if (e.kind === "approval") steps.push(el("div", p.approved ? "step" : "step denied", `${p.approved ? "✔ aprovado" : "✖ negado"}: ${p.tool} ${summarizeInput(p.input)}`));
    else if (e.kind === "tool_result") {
      if (p.is_error) steps.push(el("div", "step denied", `erro: ${String(p.output).slice(0, 200)}`));
    } else if (e.kind === "user") {
      flush();
      box.append(el("div", "msg user", p.text));
    } else if (e.kind === "assistant") {
      flush();
      const m = el("div", p.is_error ? "msg error" : "msg");
      m.innerHTML = renderMarkdown(p.text ?? "");
      box.append(m);
    } else if (e.kind === "error") {
      flush();
      box.append(el("div", "msg error", p.message));
    } else if (e.kind === "changes") {
      flush();
      box.append(changesBlock(p.files));
    }
  }
  flush();
  liveBox = el("div", "live");
  box.append(liveBox);
  renderLive();
  if (stick) box.scrollTop = box.scrollHeight;
}

let liveBox: HTMLElement | null = null;
let liveQueued = false;

/** Só a parte em streaming; agrupada por quadro para não refazer o Markdown a cada trecho. */
function scheduleLive() {
  if (liveQueued) return;
  liveQueued = true;
  requestAnimationFrame(() => {
    liveQueued = false;
    const box = $("thread");
    const stick = box.scrollTop + box.clientHeight >= box.scrollHeight - 40;
    renderLive();
    if (stick) box.scrollTop = box.scrollHeight;
  });
}

function renderLive() {
  if (!liveBox) return;
  liveBox.replaceChildren();
  const live = state.current && state.live.get(state.current.id);
  if (!live) return;
  if (live.steps.length) {
    const d = el("details", "work") as HTMLDetailsElement;
    d.open = true;
    d.append(el("summary", undefined, `Trabalhando… ${live.steps.length} ${live.steps.length === 1 ? "ação" : "ações"}`), ...live.steps.map((s) => el("div", "step", s)));
    liveBox.append(d);
  }
  if (live.text) {
    const m = el("div", "msg");
    m.innerHTML = renderMarkdown(live.text);
    liveBox.append(m);
  }
}

function changesBlock(files: { path: string; added: number | null; deleted: number | null }[]): HTMLElement {
  const box = el("div", "changes");
  box.append(el("strong", undefined, `${files.length} ${files.length === 1 ? "arquivo alterado" : "arquivos alterados"}`));
  for (const f of files) {
    const row = el("div", "file");
    row.append(el("span", "add", `+${f.added ?? "-"}`), " ", el("span", "del", `-${f.deleted ?? "-"}`), " ", f.path);
    box.append(row);
  }
  return box;
}

function renderApprovals() {
  const box = $("approvals");
  box.replaceChildren();
  const t = state.current;
  const list = (t && state.live.get(t.id)?.approvals) || [];
  for (const request of list) {
    const row = el("div", "approval");
    row.append(el("strong", undefined, request.tool), el("span", "what", summarizeInput(request.input)));
    const answer = (approved: boolean) =>
      guard(async () => {
        await api.respondApproval(t!.id, request.id, approved);
        const live = state.live.get(t!.id);
        if (live) live.approvals = live.approvals.filter((r) => r.id !== request.id);
        renderApprovals();
      });
    const yes = el("button", "primary", "Permitir") as HTMLButtonElement;
    const no = el("button", undefined, "Negar") as HTMLButtonElement;
    yes.onclick = answer(true);
    no.onclick = answer(false);
    row.append(yes, no);
    box.append(row);
  }
}

function renderComposer() {
  const busy = !!state.current && state.live.has(state.current.id);
  $("stop").hidden = !busy;
  ($("send") as HTMLButtonElement).disabled = !state.current || busy;
}

function renderAll() {
  renderSessions();
  renderContext();
  renderThread();
  renderApprovals();
  renderComposer();
}

// ---------- inspetor ----------

async function refreshGit() {
  const box = $("git");
  const cwd = state.current?.cwd;
  box.replaceChildren();
  if (!cwd) return void box.append(el("div", "empty", "Sem pasta de projeto."));
  try {
    const [files, diff] = await Promise.all([api.gitStatus(cwd), api.gitDiff(cwd, false)]);
    if (!files.length) return void box.append(el("div", "empty", "Árvore limpa."));
    for (const f of files) box.append(el("div", "file", `${f.index}${f.worktree} ${f.path}`));
    const pre = el("pre");
    for (const line of diff.split("\n")) {
      pre.append(el("div", line.startsWith("+") && !line.startsWith("+++") ? "diff-add" : line.startsWith("-") && !line.startsWith("---") ? "diff-del" : "", line || " "));
    }
    box.append(pre);
  } catch {
    box.append(el("div", "empty", "Esta pasta não é um repositório Git."));
  }
}

// ---------- eventos do núcleo ----------

function onAgentEvent({ threadId, event }: AgentEventPayload) {
  const live = state.live.get(threadId) ?? { text: "", steps: [], approvals: [] };
  state.live.set(threadId, live);
  switch (event.type) {
    case "text_delta": live.text += event.data; break;
    case "tool_use": live.steps.push(`${event.data.name} ${summarizeInput(event.data.input)}`); break;
    case "approval_requested": live.approvals.push(event.data); break;
  }
  if (threadId === state.current?.id) {
    scheduleLive();
    renderApprovals();
    renderComposer();
  }
}

async function onTurnFinished(threadId: string, isError: boolean, text: string) {
  state.live.delete(threadId);
  if (isError && text) toast(text);
  if (threadId === state.current?.id) {
    await reload(threadId);
    renderAll();
    await refreshGit();
  } else renderSessions();
}

// ---------- ações ----------

async function send() {
  const t = state.current;
  const prompt = ($("prompt") as HTMLTextAreaElement).value.trim();
  if (!t || !prompt) return;
  state.live.set(t.id, { text: "", steps: [], approvals: [] });
  state.events.push({ seq: -1, kind: "user", payload: { text: prompt }, created_at: Date.now() });
  ($("prompt") as HTMLTextAreaElement).value = "";
  renderAll();
  try {
    await api.sendMessage(t.id, prompt, ($("mode") as HTMLSelectElement).value, ($("model") as HTMLInputElement).value, ($("effort") as HTMLSelectElement).value);
  } catch (e) {
    state.live.delete(t.id);
    await reload(t.id);
    renderAll();
    throw e;
  }
}

function toggleTheme() {
  const dark = matchMedia("(prefers-color-scheme: dark)").matches;
  const current = document.documentElement.dataset.theme ?? (dark ? "dark" : "light");
  const next = current === "dark" ? "light" : "dark";
  document.documentElement.dataset.theme = next;
  try { localStorage.setItem("rusi-theme", next); } catch {}
}

async function init() {
  api.onAgentEvent(onAgentEvent);
  api.onTurnFinished((e) => void guard(() => onTurnFinished(e.threadId, e.isError, e.text))());
  api.onSessionsChanged(guard(refreshSessions));

  state.providers = await api.listProviders();
  const select$ = $("new-provider") as HTMLSelectElement;
  for (const p of state.providers) {
    const o = el("option", undefined, `${p.display_name}${p.installed ? "" : " (não instalado)"}`) as HTMLOptionElement;
    o.value = p.id;
    o.disabled = !p.installed;
    select$.append(o);
  }

  $("new-session").onsubmit = guard(async (ev) => {
    ev.preventDefault();
    const cwd = ($("new-cwd") as HTMLInputElement).value.trim() || null;
    const thread = await api.createSession(select$.value, cwd);
    await refreshSessions();
    await select(thread);
  });
  $("pick-cwd").onclick = guard(async () => {
    const dir = await api.pickFolder();
    if (dir) ($("new-cwd") as HTMLInputElement).value = dir;
  });
  $("worktree-form").onsubmit = guard(async (ev) => {
    ev.preventDefault();
    const branch = ($("branch") as HTMLInputElement).value.trim();
    if (!state.current || !branch) return;
    await api.createWorktree(state.current.id, branch);
    ($("branch") as HTMLInputElement).value = "";
    await refreshSessions();
    void guard(refreshGit)();
  });
  $("composer").onsubmit = guard(async (ev) => {
    ev.preventDefault();
    await send();
  });
  $("prompt").onkeydown = (ev) => {
    if (ev.key === "Enter" && (ev.ctrlKey || ev.metaKey)) ($("composer") as HTMLFormElement).requestSubmit();
  };
  $("stop").onclick = guard(async () => void (state.current && (await api.stopSession(state.current.id))));
  $("refresh").onclick = guard(refreshGit);
  $("theme").onclick = toggleTheme;

  await refreshSessions();
  renderAll();
}

init().catch((e) => toast(escapeHtml(String(e))));
