// Extensão do Pi carregada pelo Rusi: pede confirmação antes de qualquer ferramenta que não seja de leitura.
// Dentro do modo RPC o `confirm` vira um `extension_ui_request` que o Rusi mostra ao usuário.
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

const READ_ONLY = new Set(["read", "grep", "find", "ls"]);

export default function (pi: ExtensionAPI) {
	pi.on("tool_call", async (event, ctx) => {
		if (READ_ONLY.has(event.toolName)) return undefined;
		const ok = await ctx.ui.confirm(event.toolName, JSON.stringify(event.input));
		return ok ? undefined : { block: true, reason: "negado pelo usuário" };
	});
}
