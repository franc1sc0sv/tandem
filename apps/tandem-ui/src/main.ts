// Ventana principal: Svelte 5 con el shell de v0.3 (barra lateral y pantallas).
import "./styles/fonts";
import "./styles/global.css";

import { mount } from "svelte";
import App from "./App.svelte";
import { chooseSource, inTauri } from "./lib/source";
import { Store } from "./lib/store/store.svelte";

const target = document.getElementById("app");
if (target) {
  const source = await chooseSource(location.search, inTauri());
  mount(App, { target, props: { store: new Store(source, window.location) } });
}
