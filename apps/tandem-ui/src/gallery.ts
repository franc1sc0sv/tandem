// Galería de componentes para el arnés (solo en builds con modo demo).
import "./styles/fonts";
import "./styles/global.css";

import { mount } from "svelte";
import Gallery from "./Gallery.svelte";

const target = document.getElementById("app");
if (target) mount(Gallery, { target });
