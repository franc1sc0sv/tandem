// Ventana principal: Svelte 5 con el shell de v0.3 (barra lateral y pantallas).
import "./styles/fonts";
import "./styles/global.css";

import { mount } from "svelte";
import App from "./App.svelte";
import { DEMO_NOTICES } from "./lib/notices";
import { chooseSource, inTauri } from "./lib/source";
import { Store } from "./lib/store/store.svelte";

const target = document.getElementById("app");
if (target) {
  const source = await chooseSource(location.search, inTauri());
  const store = new Store(source, window.location);
  // `?demo&notices`: la pila de notices de ejemplo, del más viejo al más nuevo para que quede arriba el último.
  if (source.demo && new URLSearchParams(location.search).has("notices")) for (const spec of [...DEMO_NOTICES].reverse()) store.notify({ ...spec, hold: true });
  mount(App, { target, props: { store } });
}
