# Rusi - Tauri 2

Workspace desktop nativo para agentes de código, escrito em Rust. O Rusi **não tem modelo nem loop de
agente próprios**: ele hospeda os CLIs que o usuário já assina (Claude Code, Codex, Antigravity, Grok,
Cursor, Muse, OpenCode, Pi…) reutilizando o login de cada um, lado a lado, com projetos, Git, terminal
e revisão de mudanças numa janela só.

Referência de produto e de design: [PandaCode](https://github.com/aicodeking/pandacode) (Tauri 2 + Rust +
SQLite + TypeScript sem framework). Usamos como mapa de *como funciona* e de *como deve parecer*; o
código e a identidade visual são do Rusi. Não copiar código dele (a licença exige crédito em forks);
a licença do Rusi é decidida à parte.

## Princípios

- **Traga sua assinatura.** O login é sempre o do CLI oficial. O Rusi lê o estado, nunca exibe nem
  armazena tokens na UI. Chaves de provedores por API ficam no keyring do SO.
- **Um driver nativo por provedor.** Cada CLI fala o próprio protocolo (stream-json, app-server,
  MSP, RPC, ACP). Tudo vira `AgentEvent`; o resto do app não conhece provedor.
- **Um processo por turno ativo**, encerrado no fim (grupo de processos), retomado pelo id de sessão
  do provedor. Nada de agente ou watcher rodando em segundo plano sem motivo.
- **Local primeiro.** SQLite local, IPC nativo, sem servidor HTTP em produção, sem conta, sem telemetria.
  Prompts vão só para o CLI que o usuário escolheu.
- **Seguro por padrão.** Caminhos canonicalizados e contidos no projeto, Git/`gh` sempre por argv
  (nunca shell), saída e tempo limitados, nada de `--force`. Conteúdo de provedor é texto não confiável
  (Markdown sanitizado). Aprovações explícitas; modos elevados só quando o usuário escolhe para aquela tarefa.
- **Leve.** Webview do sistema, sem Chromium embutido, sem Node residente. Transcrição limitada
  (últimos 300 eventos), buffers de saída com teto, terminal e diff carregados sob demanda.

## Arquitetura

```
UI (TypeScript puro, webview do sistema)
   │  invoke(comando) / eventos "agent-event", "sessions-changed"
Tauri 2 (src-tauri/)        ← camada fina: comandos IPC, janelas, estado
   │
rusi (crate lib, raiz)      ← todo o núcleo, testável sem UI
   ├─ provider/   ProviderDriver por CLI → AgentSession → AgentEvent
   ├─ session/    run_turn: prompt → eventos → histórico + id de retomada
   ├─ persistence/ SQLite (migrations por user_version, WAL, 0600)
   ├─ secrets/    keyring do SO
   ├─ rpc/        JSON-RPC sobre stdio de processo filho
   ├─ tools/      encerra o grupo de processos de um agente
   ├─ git         status, diff, worktrees, mudanças por turno (argv)
   └─ session/approvals  PendingApprovals: pedido → resposta ao provedor
```

O CLI (`rusi "prompt"`, `rusi -c`, REPL) continua existindo e usa a mesma lib: é o primeiro cliente
e o mais barato de testar.

### Layout alvo

```
Cargo.toml          workspace: [package] rusi (lib+bin) + members = ["src-tauri"]
src/                núcleo (já existe)
src-tauri/          app Tauri 2: main.rs, commands/, capabilities/, tauri.conf.json
ui/                 TypeScript + Vite: main.ts, views, estilos, mocks de preview
docs/               contrato IPC, aprovações, worktrees, design
```

## Modelo de dados (conceitos)

- **Project**: pasta com repositório.
- **Task / Thread**: conversa com um provedor num projeto; guarda provedor, modelo, esforço, modo
  de aprovação, `worktree_path` (ou nulo = checkout principal), branch, estágio
  (`pending → in_progress → review → completed`) e id de retomada do provedor.
- **Event**: linhas sequenciais da thread (user, assistant, tool_use, tool_result, approval, changes).
- **Turn changes**: arquivos escritos pelo turno com contagem +/- e patch salvo no SQLite.

## Modos de aprovação

Três modos (`AgentMode`) mais a flag `dangerously_skip_permissions` ("full"), mapeados para o
mecanismo nativo de cada CLI (não simulamos cliques). O padrão é `accept-edits`.

| Modo | Significado | Pedidos sensíveis |
| --- | --- | --- |
| `accept-edits` | Padrão: edições passam, o resto pergunta | viram `ApprovalRequested`; sem resposta = negado |
| `auto` | Aceita tudo dentro do sandbox do provedor | não perguntam |
| `plan` | Só leitura/planejamento | negados |
| flag `dangerously_skip_permissions` | Sem prompts nem sandbox | não perguntam |

Mudar modo/modelo exige tarefa ociosa; o próximo turno reaplica a política mesmo ao retomar.

## Design da interface (inspirado no PandaCode, identidade própria)

- Três colunas: **tarefas** (agrupadas Ativas <24h / Inativas, filtro por projeto), **conversa +
  composer**, **inspetor** recolhível (Mudanças, Arquivos, Terminal).
- Atividade de ferramentas agrupada por turno num bloco recolhido "Trabalhou por …"; resposta final,
  erros e aprovações ficam fora dele.
- Barra de contexto acima do prompt: projeto · Local/Worktree · branch · seletor de provedor/modelo/esforço/modo.
- Aprovações como barra fina acima do prompt (uma linha cada, "aprovar todas").
- Modo claro e escuro, escolha salva e aplicada antes de montar a UI; respeitar transparência/contraste reduzidos.
- Cores só por tokens CSS (`--accent`…), nunca literais; movimento mínimo.
- Alternância Conversa ↔ Quadro (colunas de estágio) usando as mesmas tarefas.

## Contrato IPC (rascunho, nomes em snake_case, args em camelCase)

`list_projects`, `add_project`, `list_sessions`, `create_session`, `configure_session`,
`send_message`, `stop_session`, `respond_approval`, `get_messages`, `list_providers`,
`list_provider_models`, `git_status`, `git_diff`, `create_worktree`,
`remove_worktree`, `set_task_stage`. Erros rejeitam com `string`. Eventos: `agent-event`,
`sessions-changed`. Cada comando novo ganha um mock de preview para testar a UI no navegador.
