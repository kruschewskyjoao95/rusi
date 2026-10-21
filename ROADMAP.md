# Rusi - Tauri 2 — Roadmap

Ordem pensada para que cada fase seja usável sozinha pelo CLI e depois reaproveitada pela UI.
Ver `PROJECT.md` para arquitetura e princípios.

## Fase 0 — Base (feito)

- [x] CLI (`clap`), config TOML, `ProviderDriver` + `ProviderSnapshot`, detecção de CLIs e auth.
- [x] SQLite com migrations, threads/eventos, `--continue`; keyring (`rusi secrets`).
- [x] `AgentSession`/`AgentEvent` neutros + `run_turn` com persistência e retomada.
- [x] Drivers com sessão: Claude (stream-json), Codex (app-server), Muse (MSP), Pi (RPC),
      Grok (ACP), Antigravity (`agy -p`), Cursor, OpenCode.
- [x] Antigravity pelo `agy` oficial (um processo por turno, como o Cursor); o login é do próprio `agy`.
- [ ] Aprovações do Antigravity: em `-p` o `agy` roda tudo (`always-proceed`); `accept-edits` usa `--sandbox` em vez de perguntar.

## Fase 1 — Aprovações e modos (Claude, Codex e ACP feitos)

- [x] `AgentEvent::ApprovalRequested(ApprovalRequest { id, tool, input })` e
      `AgentSession::respond_approval(id, approved)` (padrão: `NotReady`).
- [x] Claude: `control_request` `can_use_tool` (`--permission-prompts host --permission-prompt-tool stdio`).
- [x] Codex: `item/*/requestApproval` repassado ao usuário em `accept-edits`.
- [x] ACP (Grok): `session/request_permission` → evento.
- [x] `run_turn` recebe um *approver*; o CLI pergunta no terminal (`[s]im / [N]ão`); sem terminal nega.
- [x] Testes com agentes falsos por protocolo; Claude validado ao vivo.
- [x] Muse, Pi e OpenCode perguntam em `accept-edits` (o Pi por uma extensão que o Rusi carrega, `src/provider/pi_gate.ts`).
- [ ] Cursor ainda usa o aceitar/negar fixo do modo (o `--print` do CLI não tem como perguntar no meio do turno).
- [ ] Modo `plan` negando destrutivo de forma absoluta em todos os provedores.

## Fase 2 — Git, worktrees e mudanças por turno (núcleo e CLI feitos)

- [x] Módulo `git`: `status`, `diff` — sempre argv, stdin nulo, timeout de 30 s, saída limitada, sem `--force`.
      `stage`, `commit`, `branches`, `worktree_status` e `recreate_worktree` foram removidos por falta de uso;
      voltam quando o inspetor precisar.
- [x] Worktrees: `create_worktree`, `remove_worktree` (sem `--force`); pasta `<projeto>-worktrees/<branch>`,
      branch validada.
- [x] Migration 005 (`threads.worktree_path`, `branch`); `attach_worktree` também move o `cwd` da thread,
      então agente, fs e git resolvem o checkout da tarefa.
- [x] Mudanças por turno via fotos de árvore com índice temporário (não toca índice nem refs do usuário);
      evento `changes` com +/- e patch (256 KiB/arquivo, 200 arquivos, 2 MiB/turno).
- [x] CLI: `rusi worktree new|list|rm`, `rusi git status|diff`, `rusi changes <sessão>`.
- [ ] Teto de 1 MiB por arquivo *antes* do diff (hoje só o patch é cortado).
- [ ] Estado "mesclado" do worktree e `attach` de branch existente.

## Fase 3 — Shell Tauri 2 (primeira versão feita)

- [x] Workspace Cargo: `src-tauri/` (`rusi-app`) depende da lib `rusi`; `ui/` com Vite + TypeScript puro.
- [x] Comandos IPC: `list_providers`, `list_sessions`, `create_session`, `delete_session`, `get_messages`,
      `send_message`, `stop_session`, `respond_approval`, `git_status`, `git_diff`, `create_worktree`;
      eventos `agent-event`, `turn-finished`, `sessions-changed`; erros como `string`.
- [x] UI: tarefas (Ativas/Inativas), conversa em streaming com ferramentas recolhidas, barra de
      aprovações, composer com modo (aceitar edições / auto / plano / completo) e parar, inspetor de
      mudanças (status + diff), tema claro/escuro por tokens (`prefers-color-scheme`, contraste e
      movimento reduzidos), Markdown mínimo escapado, mock de preview fora do Tauri.
- [x] Testes: `npm test` (escape/Markdown), fluxo de aprovação verificado no mock via Chromium headless.
- [ ] Seletor de modelo/esforço e de provedor por tarefa; título automático; botão de worktree na UI.
- [ ] Diff por turno (evento `changes`) abrindo o patch; árvore de Arquivos; barra de contexto editável.
- [ ] Testes Playwright no CI; empacotamento AppImage/deb (`cargo tauri build`) e CI; macOS/Windows depois.

Rodar: `cd ui && npm install`, depois `cargo install tauri-cli --version '^2'` e `cargo tauri dev`
(ou, sem o CLI, `npm run dev` em `ui/` e `cargo run -p rusi-app`).

## Fase 4 — Fluxo de trabalho

- [ ] Quadro de tarefas (Pending → In progress → Review → Completed), arquivamento após 7 dias.
- [ ] Fila durável de mensagens e *steering* (Claude por stream input, Codex `turn/steer`).
- [ ] Terminal PTY sob demanda; busca na thread (Ctrl+F); anexos (imagens, arquivos, pastas).
- [ ] Troca de provedor no meio da conversa com handoff de histórico limitado (200 msgs / 64 KiB).
- [ ] Fork de conversa a partir de uma mensagem.

## Fase 5 — Times e observabilidade

- [ ] Arquiteto + até 3 workers misturando provedores/modelos, via ferramentas MCP do host.
- [ ] Uso e limites por provedor (opcional, sem ler tokens por padrão); status "limitado" e retomada no reset.
- [ ] Atualização de CLIs com um clique; diagnóstico de instalação.

## Depois / opcional

GitHub (PRs e issues via `gh`), navegador embutido, bots (Telegram/Slack/Discord), VMs e sandboxes
em nuvem, simuladores iOS/Android, tarefas agendadas, economia de tokens.

## Fora de escopo

Servidor multi-cliente, app mobile, modelo ou loop de agente próprios, telemetria.
