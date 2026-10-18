# T3-Code — Reverse Engineering (REA, análise estática local)

> Alvo: `T3-Code-0.1.0-x86_64.AppImage` (157 MB) · App: `t3code` 0.1.0 (`t3codeCommitHash`
> `81493efb71e8`) · Método: REA (este repositório) + leitura direta do ASAR extraído.
> Nada foi executado: nenhuma análise dinâmica, nenhum login realizado, nenhum
> token lido. Todas as conclusões vêm de bytes observados no artefato.

## 1. O que é o T3-Code

Descrição oficial no próprio `package.json`:

> *"T3 Code is an open-source desktop app for coding agents. Work with your
> existing agent subscriptions, review code changes, and run commands in your
> projects. Connect from desktop, web, or mobile to continue working remotely."*

Modelo: **traga sua própria assinatura (BYOS)**. O app não vende modelo de IA;
ele reutiliza os logins e planos que o usuário já possui — Google/Antigravity,
Claude (Pro/Max via Claude Code), ChatGPT (Plus/Pro via Codex), Cursor etc. —
e oferece shell Electron + servidor local + cliente web como cockpit único,
no desktop, no navegador ou no celular.

- Nome: `t3code` · versão/build `0.1.0` · licença `MIT` · autor `T3 Tools`
- Homepage: <https://t3.codes> · gerenciador: `pnpm@11.10.0>

## 2. Metodologia e reprodução

```bash
# 1. Instalar e compilar o REA (repo em /home/joao/Projects/rea)
npm ci && npm run build:cached

# 2. Extrair o AppImage SEM executar o app (só desempacota o squashfs)
cp T3-Code-0.1.0-x86_64.AppImage /tmp/t3extract/
cd /tmp/t3extract && ./T3-Code-0.1.0-x86_64.AppImage --appimage-extract
# → squashfs-root/resources/app.asar  (o app Electron de verdade)

# 3. Inventário do ASAR com o REA
node scripts/rea.mjs inspect-artifact \
  /tmp/t3extract/squashfs-root/resources/app.asar --format json > /tmp/t3-artifact.json
# → evidência ev_493e79e1779762aabd387013d3c692f465e34692ca242831da539698decc8f84

# 4. Análise profunda do grafo JS (parcial — ver §11 Limites)
node scripts/rea.mjs analyze-javascript-application \
  /tmp/t3extract/squashfs-root/resources/app.asar --format json > /tmp/t3-analysis.json

# 5. Fuses do binário Electron
node scripts/rea.mjs inspect-electron-fuses /tmp/t3extract/squashfs-root/t3code
# → evidência ev_b23c568c8f20ecd097e9cf5dfe355dfd6550db85ed3498c5fbf259416ff8b2e8

# 6. Extração total para grep (somente leitura posterior)
node -e "require('@electron/asar').extractAll('.../app.asar','/tmp/t3app')"
```

O bundle do servidor (`binCli-CUds19Wg.mjs`, 13.652.151 bytes) preserva os
caminhos-fonte originais em marcadores `//#region src/...` e
`//#region ../../packages/...`, o que permitiu mapear módulo a módulo.

## 3. Inventário do artefato

- Formato: `asar` · SHA-256 `54e2346258da31c28761984a0cca48bba5de4918e506ce65e340c2a28d000c16`
- 3.763 entradas: `apps/` 2.421 · `node_modules/` 1.317 · `patches/` 23 ·
  `package.json` + `pnpm-workspace.yaml` (monorepo pnpm empacotado)
- `apps/desktop/` (42 arquivos): `dist-electron/` (main/preloads/workers) +
  `prod-resources/` (26 arquivos de recursos)
- `apps/server/dist/`: `bin.mjs` (1.853 bytes, lançador), `binCli-CUds19Wg.mjs`
  (13,6 MB, servidor completo), `AcpMcpStdioBridge-*.mjs` (2 variantes),
  `Effect-9MQ55RzZ.mjs` (runtime funcional), `claude-history-worker.mjs` (+
  variante com hash), `client/` (2.360 assets web)
- `rea inspect` (rota de binário nativo) **recusa o ASAR corretamente**
  (`capability_unavailable`, sem Ghidra/Hopper/Java configurados) — a rota JS é
  a adequada e não exige engine nativa.

## 4. Arquitetura geral (3 camadas)

```
┌─ Shell Electron ─────────────────────────────┐  apps/desktop/dist-electron/
│ boot.cjs (123 B) → main.cjs (5.467.836 B)    │  4 janelas, 4 context-bridges
│ preload.cjs (31.911 B) + preloads extras      │  (122 membros), 270 ops IPC
└───────────────┬──────────────────────────────┘
┌─ Servidor local (Effect/TypeScript) ─────────┐  apps/server/dist/
│ binCli-*.mjs · 9 drivers · terminal · git ·  │  ACP/MCP, relay, tunnel,
│ checkpoints · review · scheduler · usage     │  persistência, telemetria
└───────────────┬──────────────────────────────┘
┌─ Cliente web (React PWA) ────────────────────┐  apps/server/dist/client/
│ index.html (19.492 B) + 2.360 assets         │  desktop = web = mobile
└──────────────────────────────────────────────┘
┌─ CLIs/agentes externos (gerenciados) ────────┐  baixados ou reutilizados
│ agy_acp_server · codex · claude · muse …     │  no host, sob ~/.t3/perfis
└──────────────────────────────────────────────┘
```

Fatos do REA sobre a superfície Electron: 4 `browser_windows`, 13
`explicit_web_preferences`, 4 `context_bridge_apis` / 122
`exposed_api_members`, IPC com 270 operações (115 canais literais, 151
dinâmicos; 218 transmissões do renderer, 42 listeners, 10 handlers no main).
Preloads dedicados: `preview-pick`, `preview-pip`, `mac-permission`.
Workers: `snapShot/` (região, acessibilidade, atalho global),
`WindowsForegroundFocusWorker`, `compileCache`, `rolldown-runtime`.

## 5. Servidor: módulos internos (`src/*`, 30 diretórios)

`assets, auth, background, checkpointing, cli, cloud, device, diagnostics,
environment, git, mcp, observability, orchestration-v2, persistence, preview,
process, project, provider, relay, review, scheduling, secrets, telemetry,
terminal, usage, utils, vcs, workspace` — com migrations versionadas em
`persistence/Migrations/*` (ex. `036_ProjectionThreadsPinned`).

Pacotes do monorepo (`packages/*`): `contracts, effect-acp,
effect-codex-app-server, provider-acp, provider-acp-registry, provider-core,
provider-cursor, provider-grok, provider-muse, provider-opencode, provider-pi,
shared, source-control-{azure-devops,bitbucket,core,forgejo,gitcafe,github,
gitlab}, ssh`. Cada provider de primeira parte é um `ProviderDriver`
(`Drivers/<Nome>Driver.ts`) registrado no array estático `BUILT_IN_DRIVERS`
(`src/provider/builtInDrivers.ts`); o que não está na lista vira snapshot
`"unavailable"` em runtime.

## 6. Providers (ordem de `BUILT_IN_DRIVERS`)

`Codex, Claude, Cursor, Grok, OpenCode, Antigravity, Pi, Muse,
AcpRegistryDriver`. Uso consolidado em `BUILT_IN_USAGE_DRIVERS` (leitores de
transcrição primeiro, depois scanners). Cada driver tem: snapshot (instalado,
versão, auth, modelos), adaptador de orquestração, sonda de uso/limites e
rotina de instalação/atualização gerenciada.

### 6.1 Antigravity — login Google + plano do usuário

Fluxo (`AntigravityAuth.ts`, `antigravityCallback.ts`,
`antigravityAuthSupport.ts`):

1. T3 abre (ou indica) a **página de login do Google** (`usesBrowser`, padrão
   `true`); o callback é um **redirect loopback**
   `http://127.0.0.1:<porta>/<caminho>` com `state` de uso único amarrado ao
   fluxo (`flowId`, expira em 5 min — `AUTH_TIMEOUT_MS = 300000`).
2. Validação estrita do callback: exatamente um `code` (ou um `error`), no
   máximo um `iss`, e esse `iss` tem que ser `https://accounts.google.com`;
   origem/caminho têm que bater com o `redirectUri` anunciado — senão
   `"Paste the complete redirect URL from the Google sign-in page."`.
3. O **servidor ACP do Google** (`agy_acp_server`, baixado — §6.1.1) troca o
   code e persiste `acp_token.json` em `<profileDir>/antigravity-acp/` no
   formato `{client_id, client_secret, refresh_token, project_id?}`.
4. Sessões ACP (`session/new`); `SUBSCRIPTION_REQUIRED` vira a mensagem
   "Google requires an eligible Antigravity subscription for this account."
   Login em outro cliente é detectado ("Sign-in is in progress in another
   client"); sign-out/auth-required limpam metadados da conta.

#### Instalação gerenciada (`AntigravityInstallation.ts`, `antigravityRelease.ts`)

- Binário oficial **`agy-acp-server-1.3.0`** de
  `https://dl.google.com/agy-extensions/releases/<so>/agy-acp-server-1.3.0-<so>-<arch>.zip`
  (linux/macos/windows × arm64/x86_64), com harness `localharness_external`
  (`.exe` no Windows, `.par` fora dele), registro de release
  `.install-complete.json` (`releaseId` sha256 + versão + bytes), download com
  timeout de 45 min, verificação de 90 s e margem de 256 MB livres.
- Métodos suportados: `oauth-personal` e `oauth-business`; sem `acp_token.json`
  a tela de limites diz "Sign in with Google to see Antigravity limits."

#### Plano e cotas (`antigravityUsageLimits.ts`, `AntigravityUsage*.ts`)

- Access token em memória (refresh via `oauth2.googleapis.com/token`, perfil via
  `www.googleapis.com/oauth2/v2/userinfo`), cache com fingerprint sha256 de
  `client_id\0refresh_token\0project_id` e renovação 60 s antes de expirar.
- Quotas nos endpoints `daily-cloudcode-pa.googleapis.com` e
  `cloudcode-pa.googleapis.com`: buckets `{bucketId, window, displayName,
  remainingFraction, resetTime, disabled?}` e modelos `{modelProvider,
  quotaInfo, weeklyQuotaInfo}`.
- Regra de apresentação: `gemini-*` → "Gemini"; **`3p-*` → "Claude & GPT
  (shared)"** — comentário no código: *"The summary names the actual shared
  buckets; Claude and GPT consume one allowance."* Janelas: `5h` = sessão
  (300 min), `weekly` = 10080 min; `usedPercent = (1 − remainingFraction)×100`.
  Contas antigas sem `window` não ganham cadência inventada.

### 6.2 Claude — reutiliza a assinatura Claude Code

- **Auth por reaproveitamento**: lê `claudeAiOauth.accessToken` do
  `.credentials.json` no config-dir do `claude` (fora do macOS; `oauthAccount`
  com `organizationUuid` no config). Sonda de capabilities por instância
  (binário + HOME, TTL 5 min) para conta e slash-commands.
- **Execução**: Claude Agent SDK spawnando o CLI `claude` instalado
  (`pathToClaudeCodeExecutable`, `Drivers/ClaudeExecutable.ts` resolve shims
  `.cmd/.bat/.ps1` e entradas do pacote npm `@anthropic-ai/claude-code`;
  `isClaudeNativeCommandPath` reconhece `~/.local/bin/claude`).
  Driver `claudeAgent` (`Drivers/ClaudeDriver.ts`), skills e dispatch próprios
  (`ClaudeSkills.ts`, `ClaudeSkillDispatch.ts`), catálogo de modelos
  (`ClaudeModelCatalog.ts` + `ClaudeModelManifest.ts`).
- **Uso e resets** (`claudeUsageLimits.ts`, `claudeResetCredits.ts`,
  `ClaudeTurnTokenUsage.ts`, `claude-history-worker.mjs`): `GET
  https://api.anthropic.com/api/oauth/usage?cedar_ember=1&skip_spend=1` com
  `anthropic-beta: oauth-2025-04-20` e user-agent `claude-cli/<versão>
  (external, cli)`; programa `cedar_ember` com grants `{id, resets_left,
  ends_at, paused, usable_now}`, resgate idempotente por `request_id`
  (`reset / already_used / not_limited / cooldown / ineligible / unavailable`).
  Falha aqui nunca quebra as barras de uso ("reads as no resets").

### 6.3 Codex — assinatura ChatGPT + CLI oficial gerenciado

- **Auth** (`CodexChatGptAuth.ts`, `CodexAuthCallback.ts`,
  `CodexChatGptHandoff.ts`): OpenID discovery em
  `https://auth.openai.com/.well-known/openid-configuration`, recurso
  `https://api.openai.com/v1`, escopo obrigatório
  `chatgpt.tokens.use.direct`; registro de cliente `oaiapp_*`, callback
  `http://127.0.0.1|localhost:<porta>/auth/callback`; multi-sessão
  (`activeClientId`, email/subject por conta), telemetria `chatgpt.*` e trava
  de sessão (`CodexChatGptSessionLock.ts`).
- **Execução** (`CodexInstallation.ts`, `CodexManagedHome.ts`,
  `CodexManagedRuntime.ts`, `CodexDriver.ts`): baixa o pacote **Rust
  `openai/codex` v0.156.1** de `github.com/openai/codex/releases/download/
  rust-v0.156.1/codex-package-<target>.tar.gz` com **SHA-256 fixado por
  plataforma** (ex. linux-x64 `x86_64-unknown-linux-musl`), manifesto de layout
  v1, `entrypoint`, registro de release ativa; args de lançamento próprios
  (`codexLaunchArgs.ts`), revert de thread (`CodexThreadRevert.ts`),
  apresentação de ferramentas (`CodexToolPresentation.ts`), instruções de
  desenvolvedor (`CodexDeveloperInstructions.ts`), elicitação MCP
  (`CodexMcpElicitation.ts`), tokens por turno (`CodexTurnTokenUsage.ts`).
- UI: `CodexSetupSection.logic-*.js`, `ChatGptUsageButton-*.js`,
  `UsageLimits-*.js`, `settings.providers-*.js`.

### 6.4 Demais drivers

- **Muse** (`packages/provider-muse`, driver `muse`, nome exibido "Muse
  Code"): usa o CLI `muse` do host do servidor T3 — "Check your Muse
  installation and run `muse login` on this T3 server host."
- **Cursor**: SDK **embutido no bundle** (`node_modules/@cursor/sdk`,
  `@cursor/sdk@1.0.35` declarado; foi o bundle que estourou o teto do REA).
- **Grok / OpenCode / Pi**: pacotes `provider-grok`, `provider-opencode`,
  `provider-pi` com drivers e leitores de uso próprios.
- **ACP genérico** (`provider-acp`, `provider-acp-registry`,
  `AcpRegistryDriver`, `AcpMcpStdioBridge-*.mjs`): registra servidores ACP
  arbitrários (stdio) — a porta de entrada de qualquer outro agente.

## 7. Ferramentas e capacidades (as 12 dependências)

| Dependência | Para que serve no T3 |
|---|---|
| `node-pty` (^1.2.0-beta.15) + `pty.node` (x64, arm64, musl) | Terminal integrado real (PTY) |
| `playwright-core` (1.60.0) | Automação/controle de browser |
| `@ff-labs/fff-node` + bins `fff-bin-linux-x64-{gnu,musl}` | Captura de tela / framebuffer (workers de snapshot) |
| `@crowecawcaw/xa11y` + `xa11y.linux-x64-gnu.node` | Árvore de acessibilidade (localizar controles na tela) |
| `@napi-rs/keyring` + `.node` | Cofre de segredos do SO (tokens fora do disco) |
| `@clerk/electron-passkeys`, `electron-webauthn` | Passkeys/WebAuthn no desktop |
| `ffi-rs` + `.node` (gnu/musl) | Chamadas nativas (foco de janela etc.) |
| `@cursor/sdk` (1.0.35) | Integração Cursor embutida |
| `undici` | Cliente HTTP do servidor |
| `patch-package`-style `patches/` (23 patches) | Correções sobre deps (ex. `@effect/vitest`, `expo`, `react-navigation`) |

Recursos do servidor além dos agentes: `terminal/`, `process/`, `git/` +
`vcs/` + `source-control-*` (GitHub, GitLab, Bitbucket, Forgejo, Azure DevOps,
GitCafe, SSH), `checkpointing/` (snapshots e restore), `review/`
(revisão de diffs), `scheduling/`, `preview/` (preview-pick/pip),
`secrets/` + keyring, `mcp/` + pontes ACP/MCP, `usage/` (tela de limites
agregada), `persistence/` (SQLite/migrations, threads e projeções),
`telemetry/` (`AnalyticsService`, eventos `chatgpt.*`), `environment/`,
`health`/diagnósticos (`settings.diagnostics-*`).

Remoto/mobile: `cloud/` (link, `managedTunnelStartup`, `pinnedRuntime`,
`selfUpdate`, `serviceLauncherClient`, `CliTokenManager`), `relay/`
(`AgentAwarenessRelay`, `HeldHooksWaker`), `device/` (`LocalDeviceHost`,
`SshDeviceHost`, `DeviceHubProxy`, `AgentDeviceShim`, `deviceToolVersions.json`).

## 8. Cliente web (React PWA)

`index.html` (19.492 B, `lang="en"`, `theme-color #0a0a0a`, manifesto PWA,
ícones, anti-flash de tema claro/escuro via `localStorage`), `main-*.js`
(`createRoot`), ~2.360 chunks: telas de auth (`AuthSurfaceShell`,
`BrowserManagedAuthShell`, `ElectronManagedAuthShell`,
`ProviderAuthTerminal`, `authRedirect`, `useT3ConnectAuthPrompt`),
configuração (`SettingsPanels`, `settings.providers-*`,
`CodexSetupSection.logic-*`, `ProviderModelPicker`), uso
(`UsageLimits-*`, `usage-*`, `ChatGptUsageButton-*`, `chart-*`), chat
(`_chat-*`, `ChatMarkdown-*`). É a mesma UI no Electron, no navegador e no
celular.

## 9. Electron: fuses e postura de segurança

Binário `squashfs-root/t3code` (ELF x86_64, 228.605.256 bytes, SHA-256
`ee9faf5b…e8c57`; 1 sentinel, wire v1, 9 fuses completos):

| Fuse | Estado | Implicação |
|---|---|---|
| RunAsNode | enabled | `t3code --inspect` etc. honrados |
| EnableNodeCliInspectArguments | enabled | Inspector anexável |
| EnableNodeOptionsEnvironmentVariable | enabled | `NODE_OPTIONS` lido |
| GrantFileProtocolExtraPrivileges | enabled | `file://` com privilégios extras |
| EnableCookieEncryption | disabled | cookies sem criptação Fuse |
| EnableEmbeddedAsarIntegrityValidation | disabled | ASAR sem validação de integridade |
| OnlyLoadAppFromAsar | disabled | app pode carregar fora do ASAR |
| LoadBrowserProcessSpecificV8Snapshot | disabled | snapshot padrão |
| WasmTrapHandlers | enabled | handlers de trap WASM |

Leitura: conveniência de desenvolvimento/debug acima de endurecimento —
coerente com um app que gerencia CLIs e servidores locais no host.

## 10. Como o projeto foi construído (toolchain)

- Monorepo **pnpm 11.10.0** (`pnpm-workspace.yaml`, `patches/` estilo
  patch-package), TypeScript em todo o servidor com **Effect** (services,
  layers, scopes; `fn("...")` nomeando cada construtor para tracing).
- Empacotamento: bundles `.mjs`/`.cjs` com sourcemap de regiões (`//#region`)
  e hashes de conteúdo nos nomes (`-BDT-UGsc`, `-CUds19Wg`); cliente Vite/Rollup
  (chunks com hash, `rolldown-runtime` no Electron).
- Distribuição: **AppImage** (ELF static-pie, squashfs) com Electron + ASAR;
  sem auto-update embutido no pacote analisado (update via `cloud/selfUpdate`
  no servidor).
- Versionamento de dados: migrations numeradas (`036_…`, `038_…`).

## 11. Evidências REA, cobertura e limites

- `inspect-artifact`: cobertura `complete-within-substeps`, 3.696 nós /
  3.764 ocorrências / 3.763 arestas, mesma raiz SHA-256 do grafo JS
  (`c5dc0354…`).
- `analyze-javascript-application`: **parcial por teto de recurso**
  (`resource_constraint/memory`): 2.681/3.394 arquivos retidos (2.666 JS
  parseados, 677 pulados, 0 falhas de parse; 89.118.136 bytes lidos; 10.157.399
  nós AST; 43.379 achados; 362 módulos; 15.085 nós de grafo). O estouro veio do
  bundle `node_modules/@cursor/sdk/dist/cjs/index.js` contra o orçamento de
  ~2 GB do resultado pai. Evidência parcial `ev_9b0ba9bd…` (310 MB em
  `/tmp/t3-analysis.json`).
- `inspect-analysis-view` sobre evidência retida exige sessão MCP com retenção
  (fora do CLI); usado o `summary`/`statistics` inline do JSON parcial.
- Tudo aqui é **estático**: nenhum fluxo foi executado, nenhum login feito,
  nenhum plano real consultado. Nomes de plano ("personal/business") referem-se
  a `authMethod` do driver, não a nomes comerciais do Google.

## 12. Onde continuar

- Mapear canais IPC literais (115) para recursos de UI específicos.
- Extrair o catálogo de modelos (`ClaudeModelCatalog`, `codexModelCatalog`,
  `model-manifest.json`) e o `ModelManifest` geral.
- Detalhar `ProviderCredentialStore` + keyring (onde cada token dorme em disco).
- Inspecionar `cloud/managedTunnelStartup` + `relay/` para o protocolo remoto.
- Re-rodar a análise JS excluindo `node_modules/@cursor/sdk` para fechar os
  ~700 arquivos restantes (exige orçamento maior de heap/resultado).
