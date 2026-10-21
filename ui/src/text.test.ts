import assert from "node:assert/strict";
import { test } from "node:test";
import { escapeHtml, renderMarkdown, summarizeInput } from "./text.ts";

test("html do provedor é escapado", () => {
  assert.equal(escapeHtml(`<img src=x onerror="a">`), "&lt;img src=x onerror=&quot;a&quot;&gt;");
  assert.ok(!renderMarkdown("<script>alert(1)</script>").includes("<script>"));
});

test("markdown mínimo", () => {
  assert.equal(renderMarkdown("use `ls` e **isto**"), "use <code>ls</code> e <strong>isto</strong>");
  assert.equal(renderMarkdown("a\n```rust\nlet x = 1;\n```\nb"), "a<br><pre><code>let x = 1;\n</code></pre><br>b".replace("<br>b", "b"));
});

test("resumo de entrada", () => {
  assert.equal(summarizeInput({ command: "ls -la" }), "ls -la");
  assert.equal(summarizeInput({ x: 1 }), '{"x":1}');
});
