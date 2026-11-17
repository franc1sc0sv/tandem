// Snippets de texto para pasar `children` a los componentes en las pruebas.
import { createRawSnippet, type Snippet } from "svelte";

export function text(s: string): Snippet {
  return createRawSnippet(() => ({ render: () => `<span>${s.replace(/[<&]/g, (c) => (c === "<" ? "&lt;" : "&amp;"))}</span>` }));
}
