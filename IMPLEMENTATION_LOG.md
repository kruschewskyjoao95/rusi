# Rusi — Diário de Implementação & Arquitetura Detalhada 🦀

Este documento registra detalhadamente todas as etapas de desenvolvimento do projeto **Rusi**, as decisões de design adotadas, a estrutura do código e as instruções de uso.

---

## Índice
1. [Fase 1: Fundação & Interface de Linha de Comando (CLI)](#fase-1-fundação--interface-de-linha-de-comando-cli)
   - [Passo 1: Módulo de Configuração (`src/config.rs`)](#passo-1-módulo-de-configuração-srcconfigrs)
   - [Passo 2: Interface de Linha de Comando (`src/cli.rs`)](#passo-2-interface-de-linha-de-comando-srcclirs)
2. [Fase 2: Conectores LLM & Streaming de Tokens](#fase-2-conectores-llm--streaming-de-tokens)
   - [Passo 1: Tipagem Unificada & Trait `LlmProvider` (`src/llm/`)](#passo-1-tipagem-unificada--trait-llmprovider-srcllm)
   - [Passo 2: Conector Google Gemini API & Function Calling (`src/llm/gemini/`)](#passo-2-conector-google-gemini-api--function-calling-srcllmgemini)
   - [Passo 3: Parser de Streaming SSE & Integração CLI (`src/llm/stream.rs`)](#passo-3-parser-de-streaming-sse--integração-cli-srcllmstreamrs)

---

## Fase 1: Fundação & Interface de Linha de Comando (CLI)

### Passo 1: Módulo de Configuração (`src/config.rs`)

#### 1. Objetivo
Projetar um sistema robusto, idiomático e com tipagem estática para carregar, validar e persistir configurações do agente Rusi, suportando variáveis de ambiente, múltiplos arquivos `.env` e arquivos TOML globais e locais.

#### 2. Dependências Adicionadas ao `Cargo.toml`
* `dirs = "7.0"`: Identificação confiável dos diretórios de configuração do sistema operacional (ex.: `~/.config/rusi` no Linux/macOS, `%APPDATA%\rusi` no Windows).
* `dotenvy = "0.15"`: Carregamento tolerante a falhas de arquivos `.env` no diretório de trabalho e ancestrais.
* `thiserror = "2.0"`: Definição declarativa e limpa de tipos de erro customizados (`ConfigError`).

#### 3. Tipos e Modelos Implementados
* **`AgentMode`**:
  * `Auto`: Execução autônoma contínua.
  * `Plan`: Modo somente-leitura; impede modificações no workspace e gera planos de ação.
  * `AcceptEdits` (padrão): Solicita autorização humana para cada ação de escrita ou comando potencialmente destrutivo.
* **`EffortLevel`**:
  * Níveis de raciocínio profundo (*reasoning effort*): `Low`, `Medium` (padrão), `High`.
* **`ApiKeysConfig`**:
  * Gerenciamento de chaves para múltiplos provedores: `gemini_api_key`, `anthropic_api_key`, `openai_api_key`.
  * Métodos de conveniência com resolução automática de fallback para variáveis de ambiente do sistema (`GEMINI_API_KEY`, etc.).
* **`McpServerConfig`**:
  * Cadastro de servidores MCP externos com campos `command`, `args` e `env` (variáveis de ambiente específicas do subprocesso).
* **`Config`**:
  * Estrutura principal agregando o modelo padrão (`gemini-2.0-flash`), modo, esforço, flags de segurança, chaves e mapa de servidores MCP.

#### 4. Resolução Hierárquica de Configuração (Ordem de Precedência)
A configuração é resolvida em camadas:
1. **Camada 1 (Base)**: `Config::default()`.
2. **Camada 2 (Arquivo TOML)**: Detectado automaticamente na ordem:
   - `./.rusi/config.toml` (específico do workspace atual)
   - `./rusi.toml` (raiz do projeto)
   - `~/.config/rusi/config.toml` (global do usuário)
   - Caminho arbitrário via `--config <PATH>`
3. **Camada 3 (Ambiente & .env)**: Variáveis de ambiente carregadas de `.env` ou do shell têm precedência sobre os valores do arquivo TOML:
   - `RUSI_MODEL` -> sobrepõe `default_model`
   - `RUSI_MODE` -> sobrepõe `default_mode`
   - `RUSI_EFFORT` -> sobrepõe `default_effort`
   - `RUSI_DANGEROUSLY_SKIP_PERMISSIONS` -> sobrepõe permissões
   - `GEMINI_API_KEY`, `ANTHROPIC_API_KEY`, `OPENAI_API_KEY` -> sobrepõem chaves de API

#### 5. Testes Unitários
Foram implementados e validados 4 testes no módulo `config::tests`:
* `test_default_config`: Garante valores padrão seguros e consistentes.
* `test_parse_toml_config`: Valida a desserialização de TOML com servidores MCP e chaves de API.
* `test_mode_from_str`: Valida o parsing flexível de strings para `AgentMode`.
* `test_effort_from_str`: Valida o parsing de strings para `EffortLevel`.

---

### Passo 2: Interface de Linha de Comando (`src/cli.rs`)

#### 1. Objetivo
Construir uma interface de linha de comando (CLI) declarativa, ergonômica, de alta velocidade e fortemente tipada usando a crate `clap` (derive), com suporte a flags operacionais, argumentos posicionais, parsing de variáveis de ambiente e subcomandos administrativos (`mcp`, `models`, `config`, `install`, `update`).

#### 2. Estrutura de Flags e Argumentos (`Cli`)
* **Prompt Posicional (`[PROMPT]`)**: Instrução inicial direta para o agente executar em modo comando único.
* **`-C, --config <PATH>`**: Permite apontar para um arquivo TOML customizado fora dos caminhos padrões.
* **`-m, --model <MODEL>`**: Sobrescreve o modelo ativo em tempo de execução (com suporte nativo à variável `RUSI_MODEL`).
* **`--mode <MODE>`**: Define o modo de operação (`auto`, `plan`, `accept-edits`) com validação direta via `ValueEnum`.
* **`--effort <EFFORT>`**: Nível de raciocínio desejado (`low`, `medium`, `high`).
* **`--dangerously-skip-permissions`**: Ignora o portão de segurança interativo para execuções não supervisionadas em pipelines/CI.
* **`-p, --print`**: Modo de saída crua sem animações ou interface interativa.
* **`-c, --continue`**: Flag para restaurar a sessão de diálogo anterior.

#### 3. Subcomandos Implementados (`Commands`)
* **`mcp` (`McpArgs` & `McpCommands`)**:
  * `rusi mcp add <name> <command> [args...]`: Cadastra um servidor MCP e o persiste diretamente no arquivo TOML de configuração ativo.
  * `rusi mcp remove <name>` (alias `rm`): Remove um servidor cadastrado e atualiza o arquivo de configuração.
  * `rusi mcp list` (alias `ls`): Lista todos os servidores MCP configurados, seus comandos e argumentos.
* **`models`**:
  * Exibe um catálogo dos modelos suportados (Google Gemini 2.0/2.5, Anthropic Claude 3.5/3.7, OpenAI GPT-4o/o3, Ollama local) e o status das chaves de API no ambiente atual (identifica se `GEMINI_API_KEY`, `ANTHROPIC_API_KEY`, etc. estão ativas).
* **`config` (`ConfigArgs` & `ConfigCommands`)**:
  * `rusi config path`: Exibe o caminho do arquivo de configuração atualmente detectado.
  * `rusi config show`: Exibe a configuração serializada completa em TOML.
  * `rusi config get <key>`: Retorna o valor de uma chave específica.
  * `rusi config set <key> <value>`: Altera e persiste o valor de uma chave no arquivo TOML.
* **`install`**:
  * Guia de compilação e instalação global via Cargo.
* **`update`**:
  * Verificação de versão atual do binário.

#### 4. Integração no `main.rs`
* O ponto de entrada principal foi transformado em assíncrono com `#[tokio::main]`.
* O fluxo de inicialização segue a ordem estrita:
  1. `Cli::parse()` processa os argumentos passados pelo usuário.
  2. `Config::load(cli.config)` carrega os arquivos `.env` e arquivos TOML com resolução de precedência.
  3. Se houver um subcomando, ele é despachado via `cli::handle_command` e o processo é finalizado.
  4. Se for a execução padrão do agente, `cli.merge_with_config(config)` sobrepõe a configuração com quaisquer flags fornecidas na linha de comando.
  5. É renderizado o cabeçalho estilizado via crate `colored` indicando o modelo, modo, esforço e status de segurança.

#### 5. Testes Unitários de CLI
Implementados no módulo `cli::tests`:
* `test_parse_prompt_argument`: Validação de prompts posicionais diretos.
* `test_parse_flags`: Validação de parsing de todas as flags booleanas e com valores.
* `test_parse_mcp_add_subcommand`: Validação de parsing de comandos MCP com argumentos compostos e flags trailing (`--db-path`).
* `test_cli_merge_with_config`: Validação do algoritmo de sobreposição de flags sobre a configuração base.

---

## Fase 2: Conectores LLM & Streaming de Tokens

### Passo 1: Tipagem Unificada & Trait `LlmProvider` (`src/llm/`)

#### 1. Objetivo
Criar uma camada abstrata de modelos agnóstica de fornecedor (Google, Anthropic, OpenAI, Ollama), definindo estruturas universais para mensagens, papéis de diálogo, ferramentas e chunks de streaming.

#### 2. Tipos Implementados (`src/llm/types.rs`)
* **`Role`**:
  * `User`: Mensagens do usuário.
  * `Assistant`: Respostas do modelo (mapeado para `model` no Gemini).
  * `System`: Instruções de sistema.
  * `Tool`: Respostas de ferramentas.
* **`ToolCall` & `ToolResult`**:
  * Representação desacoplada de chamadas de funções com `id`, `name`, `arguments` e `content` (JSON estruturado via `serde_json::Value`).
* **`ChatMessage`**:
  * Construtores ergonômicos `ChatMessage::user()`, `ChatMessage::assistant()`, `ChatMessage::system()`, `ChatMessage::tool_result()`.
* **`ToolDefinition`**:
  * Nome, descrição e esquema JSON dos parâmetros (`parameters`).
* **`StreamChunk`**:
  * `Text(String)`: Tokens de texto parciais para exibição imediata.
  * `FunctionCall(ToolCall)`: Chamada de função emitida pelo modelo.
  * `Usage(TokenUsage)`: Contagem de tokens (prompt, completion, total).
  * `Done { finish_reason }`: Fim da geração.

#### 3. Trait `LlmProvider` (`src/llm/provider.rs`)
* Interface assíncrona com `#[async_trait]`:
  ```rust
  #[async_trait]
  pub trait LlmProvider: Send + Sync {
      fn provider_name(&self) -> &'static str;
      fn model_name(&self) -> &str;
      async fn stream_completion(
          &self,
          system_instruction: Option<&str>,
          messages: &[ChatMessage],
          tools: &[ToolDefinition],
      ) -> Result<ChunkStream, LlmError>;
  }
  ```

---

### Passo 2: Conector Google Gemini API & Function Calling (`src/llm/gemini/`)

#### 1. Objetivo
Implementar o conector nativo para a API do Google Gemini (`gemini-2.0-flash`, `gemini-2.5-pro`) com envio autenticado via cabeçalho `x-goog-api-key` e serialização bidirecional compatível com a especificação Gemini v1beta.

#### 2. Mapeamento de Requisição (`src/llm/gemini/request.rs`)
* Conversão de `ChatMessage` para o formato `contents` do Gemini com suporte a partes `text`, `functionCall` e `functionResponse`.
* Mapeamento de instruções de sistema em `systemInstruction.parts`.
* Mapeamento de ferramentas em `tools[0].functionDeclarations`.

#### 3. Tratamento de Erros da API (`src/llm/error.rs`)
* Captura e formatação clara de erros HTTP (ex.: 400 `API_KEY_INVALID`, 429 rate limit, 503) exibindo detalhes retornados pelo backend da Google.

---

### Passo 3: Parser de Streaming SSE & Integração CLI (`src/llm/stream.rs`)

#### 1. Objetivo
Processar o fluxo de bytes HTTP recebido via Server-Sent Events (SSE) linha a linha, decodificando payloads JSON parciais e transformando-os em `StreamChunk`.

#### 2. Implementação do Parser (`parse_gemini_sse_stream`)
* Bufferização de linhas incompletas que chegam fragmentadas em múltiplos pacotes TCP.
* Extração do prefixo `data:` e deserialização em `GeminiStreamResponse`.
* Emissão de texto incremental, chamadas de ferramentas e metadados de tokens via `async_stream`.

#### 3. Integração com a Linha de Comando (`src/main.rs`)
* Execução de prompts diretos via `rusi "seu prompt"`.
* Streaming em tempo real no `stdout` com `std::io::stdout().flush()`.
* Exibição de resumo com total de tokens consumidos ao final da execução.

#### 4. Testes Unitários Validados
* `test_parse_gemini_sse_text_chunks`: Validação de buffering e decodificação SSE completa.
* `test_gemini_request_serialization`: Validação de conversão de mensagens e papéis para o formato do Gemini.
* `test_gemini_tool_definition_serialization`: Validação de declarações de funções para o Gemini.

---
