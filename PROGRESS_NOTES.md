# Rusi — Onde Paramos (2026-10-17)

## Direção do projeto
Rusi **não tem modelo nem loop de agente próprios**. Ele hospeda os agentes que o usuário já assina
(Claude, Codex/ChatGPT, Antigravity/Google, Cursor, Muse, Grok, OpenCode, Pi), reutilizando o login de
cada CLI, igual ao T3-Code (ver `t3-code-reverse-engineering.md`). Um driver nativo por provedor, no
protocolo do próprio CLI. ACP só no Antigravity e no Grok (como no T3).
Fases e arquitetura: `ROADMAP.md`. Layout: `PROJECT.md`.

## Estado
- Fases 0–3 concluídas. `cargo test` (216 testes) e `cargo clippy --all-targets -- -D warnings` limpos.
- **Nada foi commitado** (só o commit inicial). Fazer commit antes de continuar.
- O código antigo (Gemini, loop ReAct, `src/llm`) foi removido; `src/tools/{fs,process}.rs` ficou como base
  para terminal/checkpoints. `.agents/teamwork/` e os `*.md` antigos de testes (TEST_*.md, IMPLEMENTATION_LOG.md,
  ORIGINAL_REQUEST.md) estão obsoletos.

## O que existe
`src/`: `cli`, `config`, `chat` (fluxo do terminal), `session` (AgentSession/AgentEvent/run_turn),
`persistence` (SQLite, migrations 001–004), `secrets` (keyring), `rpc` (JSON-RPC em stdio),
`provider/` (um driver por provedor + `acp_session`).
Comandos: `rusi "prompt"`, `-p`, `-c`, REPL, `--provider/--model/--mode/--effort`, `providers`,
`threads list|pin|unpin|delete`, `secrets set|status|delete`.

## O que foi verificado ao vivo
Claude, Muse e OpenCode: turno, retomada, negar/aprovar ferramenta, interrupção.
Codex, Grok, Cursor: só detecção e "sem login" (usuário não está logado neles).
Pi: protocolo ok, mas o Anthropic recusa o OAuth da assinatura em app de terceiros
("draw from your extra usage"). Antigravity: binário não instalado, só testado com agente falso.
Todos têm testes de integração com agentes falsos (`tests/*_session.rs`).

## Pendências / atenção
1. Usuário rodar `codex login`, `grok login`, `cursor-agent login` e testar `rusi --provider <x> "oi"`;
   se falhar, ajustar o tradutor daquele driver (protocolos lidos do T3/esquemas, não confirmados com login).
2. Cursor: `--trust` sempre ligado; `plan`→`--plan`, `auto`/skip→`--force` (não validado com login real).
3. Pi: sem aprovações; `plan`/`accept-edits` só liberam ferramentas de leitura.
4. Antigravity: falta binário e login (fase 4). Mensagens já orientam.
5. Permissões: negadas e listadas ao fim do turno até a fase 6 (portão interativo).
6. Modo `auto` do Claude (`--permission-mode auto`) não foi testado.
7. REPL simples (sem edição/histórico) até a TUI (fase 8).

## Próximo passo
Fase 4: instalação gerenciada (download + SHA-256 fixado, registro da release) e login OAuth loopback
— Antigravity (`dl.google.com`, `state` 5 min, `iss` Google, `BROWSER` capturando a URL) e Codex
(OIDC `auth.openai.com`, callback `/auth/callback`, ou `account/login/start` do app-server).
Depois: 5 uso/limites (evitar ler token para endpoints não documentados), 6 portão de permissões,
7 PTY/git/checkpoints, 8 TUI.

## Referências úteis (fora do repo)
T3 extraído em `/tmp/t3app` (pode ter sumido); bundle do servidor: `apps/server/dist/binCli-*.mjs`,
regiões `//#region ...` por módulo. Esquemas gerados: `codex app-server generate-json-schema`,
`muse schema generate-json-schema`, `opencode serve` → `/openapi.json`.
