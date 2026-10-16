# Rusi — Orquestrador de Agentes de IA em Rust 🦀

> **Rusi** é um orquestrador de agentes de linha de comando autônomo, seguro e de altíssima performance, construído em Rust.
> Inspirado na engenharia reversa e arquitetura do **Antigravity CLI** (*Jetski*), adaptado para um design *clean-room* modular, moderno e independente de ecossistemas fechados.

---

## 1. Visão Geral da Arquitetura

O Rusi divide o fluxo de orquestração em quatro camadas desacopladas:

```mermaid
flowchart TD
    subgraph UI ["Interface CLI & Terminal (clap + reedline)"]
        CLI[Linha de Comando & Flags]
        TUI[Renderizador Streaming & Prompts]
    end

    subgraph Core ["Motor de Raciocínio (Cortex)"]
        Loop[ReAct Agent Loop]
        Context[Gerenciador de Contexto & Histórico]
        Modes[Modos: Auto / Plan / Accept-Edits]
        Gate[Portão de Permissões & Sandbox]
    end

    subgraph LLM ["Provedores de Modelo (reqwest + serde)"]
        Gemini[Google Gemini API]
        Claude[Anthropic Claude API]
        OpenAI[OpenAI / Compatíveis]
        Local[Ollama / Modelos Locais]
    end

    subgraph Tools ["Execução de Ferramentas"]
        Builtin[Ferramentas Nativas: FS, Bash, Search]
        MCPClient[Cliente MCP (JSON-RPC 2.0 stdio)]
    end

    UI --> Core
    Core <--> LLM
    Core --> Gate
    Gate --> Tools
```

---

## 2. Stack Tecnológica Rust Selecionada

| Componente | Crate Rust | Motivo da Escolha |
| :--- | :--- | :--- |
| **Runtime Assíncrono** | `tokio` (full) | Padrão da indústria para I/O assíncrono, subprocessos e timers. |
| **CLI & Argumentos** | `clap` (derive) | Interface de linha de comando declarativa com tipagem estática e auto-help. |
| **Cliente HTTP / SSE** | `reqwest` + `tokio-stream` | Streaming de tokens e respostas do LLM via Server-Sent Events (SSE). |
| **Serialização** | `serde` + `serde_json` | Manipulação de esquemas de ferramentas, JSON-RPC do MCP e payloads. |
| **Busca Rápida de Código** | `ignore` + `regex` | Motor idêntico ao do `ripgrep` (criado pelo mesmo autor) embutido nativamente. |
| **Interface Interativa** | `reedline` + `ratatui` | Edição multilinha no terminal com histórico e renderização rica de Markdown. |
| **Protocolo MCP** | `tokio::process` + `serde_json` | Comunicação IPC via stdin/stdout com servidores MCP externos. |

---

## 3. Roadmap de Implementação

### Fase 1: Fundação & Interface de Linha de Comando (CLI)
* **Objetivo**: Estruturar o projeto Cargo e criar o CLI com todas as flags e subcomandos fundamentais.
* **Tarefas**:
  - [x] Inicializar workspace Rust (`cargo init --bin`).
  - [x] Definir CLI com `clap`:
    - Flags: `--mode` (`auto`, `plan`, `accept-edits`), `--model`, `--effort` (`low`, `medium`, `high`), `--dangerously-skip-permissions`, `--print`, `-c / --continue`.
    - Subcomandos: `mcp` (add, remove, list), `models`, `install`, `update`.
  - [x] Carregamento de configuração (`~/.config/rusi/config.toml` ou `.env`).
  - [x] Suporte a chaves de API (`GEMINI_API_KEY`, `ANTHROPIC_API_KEY`, etc.).

### Fase 2: Conectores LLM & Streaming de Tokens
* **Objetivo**: Conectar aos provedores de inteligência com respostas em tempo real.
* **Tarefas**:
  - [x] Trait `LlmProvider` abstrata para desacoplar modelos:
    ```rust
    #[async_trait]
    pub trait LlmProvider: Send + Sync {
        async fn stream_completion(
            &self,
            system_instruction: Option<&str>,
            messages: &[ChatMessage],
            tools: &[ToolDefinition],
        ) -> Result<ChunkStream, LlmError>;
    }
    ```
  - [x] Implementar conector da **Google Gemini API** (Gemini 2.0 Flash / Pro) com chamada de ferramentas estruturada (`functionDeclarations`).
  - [x] Suporte a streaming com Server-Sent Events (SSE) e formatação Markdown instantânea no terminal.


### Fase 3: Conjunto de Ferramentas Nativas (*Built-in Tools*)
* **Objetivo**: Prover ao agente as capacidades essenciais para ler, editar e executar no workspace.
* **Tarefas**:
  - [ ] **Sistema de Arquivos**:
    - `file_read(path, start_line, end_line)`
    - `file_write(path, content, overwrite)`
    - `file_edit(path, target_block, replacement_block)`
  - [ ] **Execução de Comandos**:
    - `bash_run(command, cwd, timeout_ms)`: Execução assíncrona com captura de stdout/stderr e cancelamento limpo.
  - [ ] **Busca no Código**:
    - `code_search(pattern, path)`: Busca regex ultra-rápida respeitando `.gitignore` através da crate `ignore`.
  - [ ] **Portão de Segurança e Permissões (`Gate`)**:
    - Interceptar comandos destrutivos ou edições para aprovação do usuário (`[y/N]`), salvo se `--dangerously-skip-permissions` estiver ativo.

### Fase 4: O Motor do Agente (*Cortex Loop*)
* **Objetivo**: Implementar o loop autônomo ReAct (Reason + Act).
* **Tarefas**:
  - [ ] Estrutura do loop:
    1. Monta contexto atual (System Prompt + Histórico + Definição das Ferramentas).
    2. Envia para o LLM e aguarda decisão (Texto direto vs. `ToolCall`).
    3. Se houver `ToolCall`:
       - Valida permissões.
       - Executa a ferramenta e formata o resultado.
       - Adiciona resultado ao histórico do diálogo.
       - Realimenta o modelo recursivamente até atingir o objetivo ou o limite de turnos.
  - [ ] Gestão de modos:
    - `Plan`: Impede a execução de ferramentas de escrita; apenas planeja.
    - `Accept-Edits`: Solicita confirmação manual para cada ação.
    - `Auto`: Executa autonomamente até terminar a tarefa.
  - [ ] Compactação e janela de contexto para evitar estouro de tokens.

### Fase 5: Cliente MCP (Model Context Protocol)
* **Objetivo**: Permitir que o Rusi utilize qualquer ferramenta externa existente no ecossistema MCP.
* **Tarefas**:
  - [ ] Implementar transporte `stdio` assíncrono em Rust sobre JSON-RPC 2.0.
  - [ ] Descoberta de ferramentas dinâmicas (`tools/list`).
  - [ ] Tradução bidirecional entre definições MCP e as chamadas do LLM.
  - [ ] Gerenciamento de servidores no arquivo de configuração do Rusi:
    ```toml
    [mcp_servers.sqlite]
    command = "uvx"
    args = ["mcp-server-sqlite", "--db-path", "./test.db"]
    ```

### Fase 6: Experiência de Terminal & Comandos Slash
* **Objetivo**: Interface polida para desenvolvedores.
* **Tarefas**:
  - [ ] Suporte a comandos rápidos (Slash Commands): `/plan`, `/compact`, `/help`, `/clear`.
  - [ ] Histórico persistente de comandos no terminal (`~/.rusi_history`).
  - [ ] Renderização de diffs coloridos antes de aplicar alterações em arquivos.

---

## 4. Estrutura Proposta para o Projeto em Rust

```text
/home/joao/Projects/rusi/
├── Cargo.toml
├── README.md
├── ROADMAP.md
└── src/
    ├── main.rs                   # Ponto de entrada CLI e parsing de argumentos
    ├── config.rs                 # Gerenciamento de opções e variáveis de ambiente
    ├── cortex/                   # O motor de orquestração e loop ReAct
    │   ├── mod.rs
    │   ├── loop_engine.rs        # Ciclo de vida das mensagens e turnos
    │   ├── context.rs            # Gerenciamento de tokens e janela de contexto
    │   └── permissions.rs        # Gatekeeper de confirmações e segurança
    ├── llm/                      # Abstração e provedores de IA
    │   ├── mod.rs
    │   ├── provider.rs           # Trait LlmProvider
    │   ├── gemini.rs             # Implementação Google Gemini
    │   └── stream.rs             # Parser de SSE e chunks
    ├── tools/                    # Ferramentas nativas do sistema
    │   ├── mod.rs
    │   ├── fs.rs                 # Leitura, escrita e edição de arquivos
    │   ├── process.rs            # Execução de comandos bash
    │   └── search.rs             # Busca rápida com a crate ignore
    ├── mcp/                      # Cliente Model Context Protocol
    │   ├── mod.rs
    │   ├── client.rs             # Conexão stdio e protocolo JSON-RPC
    │   └── registry.rs           # Gerenciamento de múltiplos servidores MCP
    └── ui/                       # Interface de usuário e terminal
        ├── mod.rs
        ├── terminal.rs           # Leitor de prompts interactivos
        └── renderer.rs           # Exibição rica e diffs
```

---

## 5. Próximo Passo

Para começar a implementação da **Fase 1**, podemos inicializar o pacote `Cargo.toml` com as dependências essenciais e montar o esqueleto do projeto.
