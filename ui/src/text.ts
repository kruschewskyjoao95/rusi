/** Escapa HTML. Conteúdo de provedor é texto não confiável: nunca vai cru para o DOM. */
export function escapeHtml(text: string): string {
  return text.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!);
}

/** Markdown mínimo e seguro: blocos ``` , `código` e **negrito**; o resto é texto. */
export function renderMarkdown(source: string): string {
  return escapeHtml(source)
    .split(/```[^\n]*\n?/)
    .map((part, i) => (i % 2 ? `<pre><code>${part}</code></pre>` : inline(part)))
    .join("");
}

function inline(text: string): string {
  return text
    .replace(/`([^`\n]+)`/g, "<code>$1</code>")
    .replace(/\*\*([^*\n]+)\*\*/g, "<strong>$1</strong>")
    .replace(/\n/g, "<br>");
}

/** Resumo de uma linha da entrada de uma ferramenta. */
export function summarizeInput(input: unknown): string {
  if (input && typeof input === "object") {
    const o = input as Record<string, unknown>;
    const pick = o.command ?? o.file_path ?? o.path ?? o.pattern ?? o.url;
    if (typeof pick === "string") return pick;
  }
  const text = typeof input === "string" ? input : JSON.stringify(input ?? "");
  return text.length > 120 ? text.slice(0, 120) + "…" : text;
}
