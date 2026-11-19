// Ventana de la barra de menú / tray (V3-Tray).
import "./styles/fonts";
import "./styles/global.css";

import { mount } from "svelte";
import Tray, { type TrayActions } from "./Tray.svelte";
import { tauriBridge } from "./lib/bridge";
import { chooseSource, inTauri } from "./lib/source";
import { Store, TRAY_TOPICS } from "./lib/store/store.svelte";

const target = document.getElementById("app");
if (target) {
  const source = await chooseSource(location.search, inTauri());
  let actions: TrayActions = { showMain: () => {}, quit: () => {} };
  if (inTauri()) {
    const bridge = await tauriBridge();
    actions = { showMain: () => bridge.showMain(), quit: () => bridge.quit() };
  }
  mount(Tray, { target, props: { store: new Store(source, null, TRAY_TOPICS), actions } });
}
