// Text snippets to pass `children` to the components in tests.
import { createRawSnippet, type Snippet } from "svelte";

export function text(s: string): Snippet {
  return createRawSnippet(() => ({ render: () => `<span>${s.replace(/[<&]/g, (c) => (c === "<" ? "&lt;" : "&amp;"))}</span>` }));
}
