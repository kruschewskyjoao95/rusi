# Rusi — Host de agentes que reutiliza assinaturas

Veja `ROADMAP.md` para fases e arquitetura. Estado atual:

## Layout
```
src/
├── main.rs, lib.rs
├── cli.rs            # clap: providers, mcp, config, install, update
├── config.rs         # Config (default_provider, default_model, modo, esforço, MCP)
├── provider/
│   ├── mod.rs        # trait ProviderDriver, built_in_drivers, snapshot_all
│   ├── snapshot.rs   # ProviderSnapshot, AuthState
│   ├── claude_session.rs # stream-json do claude -> AgentEvent
│   ├── claude.rs     # ClaudeDriver: binário, versão, auth via .credentials.json
│   ├── codex*.rs, muse*.rs, pi*.rs, grok.rs, antigravity.rs, opencode*.rs, cursor*.rs
│   ├── acp_session.rs # cliente ACP (Grok e Antigravity)
│   └── cli_probe.rs  # resolve_binary, versão
├── chat.rs           # fluxo do terminal: thread, sessão, streaming
├── session/          # AgentSession, AgentEvent, run_turn
├── rpc/              # RpcPeer: JSON-RPC sobre stdio de um processo filho
├── persistence/      # Database (SQLite), migrations, threads e eventos
├── secrets/          # SecretStore: KeyringStore, MemoryStore
└── tools/            # fs.rs, process.rs (base para terminal/checkpoints)
```

## Contrato
```rust
#[async_trait]
pub trait ProviderDriver: Send + Sync {
    fn id(&self) -> &'static str;
    fn display_name(&self) -> &'static str;
    async fn snapshot(&self) -> ProviderSnapshot;
}
```
Próximo: adicionar sessão (`start/send/interrupt/stream`) ao trait quando o cliente ACP existir.
