// Main window: Svelte 5 with the v0.3 shell (sidebar and screens).
import "./styles/fonts";
import "./styles/global.css";

import { mount } from "svelte";
import App from "./App.svelte";
import { chooseSource, inTauri } from "./lib/source";
import { Store } from "./lib/store/store.svelte";

const target = document.getElementById("app");
if (target) {
  const source = await chooseSource(location.search, inTauri());
  const store = new Store(source, window.location);
  mount(App, { target, props: { store } });
}
