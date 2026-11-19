// Component gallery for the harness (only in builds with demo mode).
import "./styles/fonts";
import "./styles/global.css";

import { mount } from "svelte";
import Gallery from "./Gallery.svelte";

const target = document.getElementById("app");
if (target) mount(Gallery, { target });
