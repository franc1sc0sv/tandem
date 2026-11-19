// Main window: Svelte 5 with the v0.3 shell (sidebar and screens).
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
  // `?demo&notices`: the sample notice stack, from oldest to newest so the last one ends up on top.
  if (source.demo && new URLSearchParams(location.search).has("notices")) for (const spec of [...DEMO_NOTICES].reverse()) store.notify({ ...spec, hold: true });
  mount(App, { target, props: { store } });
}
