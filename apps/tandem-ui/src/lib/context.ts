import { getContext, setContext } from "svelte";
import type { Store } from "./store/store.svelte";

export const STORE_KEY = Symbol("tandem-store");

export function provideStore(store: Store): void {
  setContext(STORE_KEY, store);
}

export function useStore(): Store {
  const s = getContext<Store | undefined>(STORE_KEY);
  if (!s) throw new Error("falta el store de Tandem en el contexto");
  return s;
}
