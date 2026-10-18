# Rusi — Host de Agentes de Código em Rust 🦀

Rusi **não tem modelo nem loop de agente próprios**. Ele hospeda os agentes que o
usuário já assina (Claude, ChatGPT/Codex, Google/Antigravity, Cursor, Muse…), reutilizando
o login de cada um — o mesmo modelo "traga sua assinatura" do T3-Code
(ver `t3-code-reverse-engineering.md`).

## Arquitetura

```
Cliente (CLI/TUI; depois web) ──► rusi (Rust, tokio)
                                   ├─ ProviderDriver por provedor
                                   │    snapshot (instalado / versão / auth)
                                   │    sessão (start / send / interrupt / stream)
                                   │    instalação gerenciada + checksum
                                   │    sonda de uso e limites
                                   ├─ Cliente ACP (JSON-RPC stdio) + ponte MCP
                                   ├─ Cofre de segredos (keyring do SO)
                                   ├─ Persistência (SQLite + migrations)
                                   └─ Terminal PTY, git, checkpoints
                                          │ spawn
                      claude · codex · agy_acp_server · cursor-agent · muse · …
```

## Princípios

- O login é sempre o do CLI oficial; o Rusi lê estado, nunca expõe tokens.
- Spawnar o CLI oficial é o caminho padrão. Ler tokens para endpoints de uso é opcional
  (endpoints não documentados podem quebrar ou violar termos).
- `Plan` bloqueia ações destrutivas de forma absoluta; aprovações vêm dos pedidos ACP dos agentes.

## Fases

- [x] **0. Base** — CLI (`clap`), config TOML, trait `ProviderDriver`, `ProviderSnapshot`,
      detecção de CLIs, auth do Claude via `.credentials.json`, `rusi providers`.
- [x] **1. Persistência e segredos**
  - SQLite (`src/persistence`): migrations versionadas por `PRAGMA user_version` (rejeita banco
    mais novo), WAL, FK, arquivo 0600 em `~/.local/share/rusi/rusi.db` (`RUSI_DB_PATH`).
    Threads com eventos sequenciais, fixar, renomear, apagar em cascata, `--continue`.
  - Segredos (`src/secrets`): trait `SecretStore`, keyring do SO (Secret Service) e store em memória.
    `rusi secrets set|status|delete` (valor só por stdin, nunca exibido).
  - `rusi threads list|pin|unpin|delete`.
- [x] **2. Claude** — sessão persistente via `claude --print --input-format stream-json
      --output-format stream-json` (um processo por conversa, vários turnos).
  - `src/session`: `AgentSession`/`AgentEvent` neutros de provedor + `run_turn`, que grava o
    histórico na thread e guarda o id de sessão do provedor (migration 004) para `--resume`.
  - Streaming de texto, raciocínio, ferramentas e resultados; tokens e custo por turno.
  - Interrupção (Ctrl-C → `control_request: interrupt`) mantém a sessão aberta; `close` faz EOF e,
    se preciso, derruba o grupo de processos.
  - Permissões: `--permission-prompts none` — o que exigiria aprovação é negado e reportado
    até a fase 6. Modo `plan`/`accept-edits`/`auto` e `--dangerously-skip-permissions` mapeados.
  - `rusi "prompt"`, `rusi -p "prompt"`, `rusi -c`, e REPL sem prompt.
- [x] **3. Um driver nativo por provedor, como no T3** (nada de "ACP genérico": o T3 só usa
      ACP no Antigravity e no Grok; cada um dos outros fala o protocolo do próprio CLI).
  - Codex: `codex app-server` (JSON-RPC sem campo `jsonrpc`), `thread/start|resume`, `turn/start|interrupt`,
    aprovações do servidor; login lido por `account/read`.
  - Muse: `muse serve` (MSP, JSON-RPC 2.0, UUIDv7), `session/start|resume`, `approval/requested` +
    `approval/decide`.
  - Pi: `pi --mode rpc` (JSONL), turno termina em `agent_settled`; sem aprovações no Pi, então
    `plan`/`accept-edits` limitam as ferramentas a leitura. Login por `pi auth check`.
  - Grok: `grok agent stdio` (ACP), modos por `--permission-mode`/`--always-approve`.
  - Antigravity: `agy_acp_server` (ACP) com perfil privado (`GEMINI_HOME`), `authenticate`
    `oauth-personal`, `session/resume`, modos `default`/`yolo`. Falta instalar o binário e logar (fase 4).
  - OpenCode: `opencode serve` (v2) com senha gerada, REST `/api/session/*` + SSE `/api/event`.
  - Cursor: `cursor-agent -p --output-format stream-json`, um processo por turno (`--resume`).
    O T3 usa o `@cursor/sdk`, que não tem equivalente em Rust.
  - Infra: `src/rpc` (par JSON-RPC em stdio) e `inject` para ordenar o fim de pedidos ACP.
- [ ] **4. Instalação gerenciada e login OAuth loopback**
  - Antigravity: `agy-acp-server` de `dl.google.com`, redirect `127.0.0.1`, `state` de uso
    único (5 min), `iss` = `https://accounts.google.com`.
  - Codex: release `openai/codex` com SHA-256 fixado, OIDC `auth.openai.com`,
    callback `/auth/callback`.
- [ ] **5. Uso e limites** por provedor.
- [ ] **6. Portão de permissões** — responde aos pedidos de aprovação dos agentes (modos
      `plan` / `accept-edits` / `auto`).
- [ ] **7. Terminal PTY, git, checkpoints, review de diffs.**
- [ ] **8. TUI, depois API para cliente web.**

## Código herdado

`src/tools/{fs,process}.rs` (leitura/escrita/edição e `bash_run` com kill de grupo) deixam de
ser ferramentas de agente; servem de base para terminal e checkpoints. O conector Gemini e o
design de loop ReAct foram removidos (histórico no git).
