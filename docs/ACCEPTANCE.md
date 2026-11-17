# Aceptación de Tandem v0.3 (definición de «terminado»)

La app está **terminada** solo cuando el agente `acceptance` (`.claude/agents/acceptance.md`) responde `ACCEPTANCE: FINAL_OK` con **todos** los ítems de este archivo en PASA, en una sola corrida sobre `origin/develop`. Nadie más (ni el capitán ni un implementador) puede declarar la app terminada.

Notación: `[hw]` = necesita el hardware real (lo cubre la suite T-123 o queda en la lista de hardware pendiente de su tarea con la razón). Cada pantalla se compara con su mockup en `mockups/v03/` (umbral de similitud **≥ 0.99**).

## 0. Reglas globales (se revisan en todas las pantallas)

- [ ] G1. Solo dos máquinas; ningún texto, botón o flujo de una tercera (D1).
- [ ] G2. Nada fijo a Mac/PC: nombres, íconos y colores salen de cada máquina; probado con un par Windows+Windows y macOS+macOS simulado (D2, A1).
- [ ] G3. Barra lateral en todas las pantallas principales: Puesto, Túneles (contador), Monitores, Acomodo, Red, Máquinas (una por máquina, con su color), Ajustes (D16).
- [ ] G4. Sistema único de botones (alto fijo, ícono siempre, elevación al pasar, brillo en el primario, foco visible); íconos en toda acción; chips de estado uniformes; tarjetas sin línea de color arriba.
- [ ] G5. Sin textos de relleno ni explicaciones de más; sin «Teclado aquí»; sin «requiere cable»; sin modos viejos.
- [ ] G6. Cada elemento interactivo hace algo real y verificable en el agente; nada decorativo que parezca botón.
- [ ] G7. **Animaciones:** cada `@keyframes` y cada `transition` del mockup de la pantalla tiene su equivalente en la app (mismo efecto, duración y curva aproximadas); entradas de tarjetas, cambios de pantalla y cambios de estado animados.
- [ ] G8. Con `prefers-reduced-motion: reduce` no hay animaciones.
- [ ] G9. Ninguna consola con errores; ningún `{{`, `undefined`, `NaN` ni texto en inglés visible.
- [ ] G10. Configuración: todo lo editable se edita desde la UI y llega a la otra máquina; el aviso «Cambiado desde X · Deshacer» funciona (D4, D5).
- [ ] G11. Calidad: fmt, clippy, tests de Rust, `pnpm test/check/build` y CI de `develop` en verde en Linux, Windows y macOS.

## 1. Pantallas (paridad visual ≥ 0.99 + todo lo interactivo funciona)

| # | Pantalla | Mockup(s) y estados a comparar |
|---|---|---|
| P1 | Puesto | `V3-Puesto` |
| P2 | Túneles | `V3-Computo`: sin túneles, proyecto plegado/desplegado, túnel con 1 y con 12 procesos, túnel cerrado |
| P3 | Máquina | `V3-Maquina`, `F-MaquinaMac` |
| P4 | Monitores y escenas | `V3-Monitores` (cada entrada seleccionada), `F-NuevaEscena` |
| P5 | Acomodo | `V3-Acomodo`: 4 lados × 3 alineaciones, cada pantalla seleccionada, pantalla no compatible |
| P6 | Prueba de cruce | `F-PruebaCruce`: esperando, cruzó bien, borde equivocado, tiempo agotado |
| P7 | Red | `V3-Red` |
| P8 | Asistente | `F-Asis1`, `V3-Asistente`, `F-Asis3`, `F-Asis4`, `F-Asis5`, `F-Asis6` |
| P9 | Progreso de escena / energía | `F-Transacciones`: en curso, éxito, vuelta atrás |
| P10 | Avisos | `F-Avisos` |
| P11 | Ajustes | `F-Ajustes`: cada pestaña |
| P12 | Paleta ⌘K | `F-Paleta` |
| P13 | Estados difíciles | `V3-Estados`: sin agente, otra máquina apagada, enlace caído, permisos revocados |
| P14 | Barra de menú / bandeja | `V3-Bandeja` |
| P15 | Lo que ve Claude | `F-ClaudeMCP` (las herramientas listadas existen en `tandem mcp`) |

Diferencias permitidas con el mockup: las de `mockups/README.md` («Lo que los mockups dicen y DECISIONS corrige») y los datos reales en lugar de los de ejemplo. Nada más.

## 2. Flujos de punta a punta (dos agentes reales por loopback + UI)

- [ ] F1. **Primer uso:** asistente 1→6 completo en las dos máquinas: nombre y color, emparejar con código de 6 dígitos (las dos UIs muestran el mismo código), permisos verificados (doble en CI; `[hw]` reales), teclado y mouse detectados en cada máquina, acomodo + prueba de cruce que pasa con un `CursorSample` real del motor, resumen comprobado y comando MCP copiable.
- [ ] F2. **Acomodo:** mover la otra máquina a cada lado y alineación, guardar, verificar en la otra máquina y en la geometría del motor de entrada (cruce simulado por el borde nuevo pasa; por el viejo no).
- [ ] F3. **Escenas:** crear, editar, aplicar con éxito, aplicar con falla inyectada → vuelta atrás visible; atajo global de escena.
- [ ] F4. **Monitores:** monitor estándar falso: cambiar entrada con veredicto; monitor de fábrica: «no compatible», sin ninguna escritura; re-elección de dueño al apagar el dueño. `[hw]` 34" real con vuelta automática.
- [ ] F5. **Túneles por MCP:** un cliente MCP abre túnel con workspace de git, corre 3 procesos a la vez, lee salida, copia archivos, abre un puerto, corta el enlace y lo recupera sin perder salida, `bring_back` crea la rama; la UI muestra todo en vivo en solo lectura. `[hw]` PC↔Mac por Wi-Fi.
- [ ] F6. **Red:** cortar el camino principal → la entrada pasa al siguiente en < 1 s, evento visible en Red; «Medir ancho de banda» da un valor; datos pausados si la entrada se degrada.
- [ ] F7. **Teclado y mouse sin dueño:** entrada física simulada en A y luego en B: manda la que se usa; sin teclas pegadas al cambiar o al caer el enlace. `[hw]` latencia real por Wi-Fi.
- [ ] F8. **Portapapeles y archivos:** copiar texto/imagen en A y pegar en B; arrastrar un archivo al cruzar el borde lo deja en B (dobles en CI; `[hw]` real).
- [ ] F9. **Configuración compartida:** cambiar nombre, color, atajos y preferencias en A → llegan a B; cambio simultáneo converge; Deshacer restaura.
- [ ] F10. **Energía:** apagar la otra máquina (transacción con progreso) y encenderla (Wake-on-LAN simulado); otra máquina perdida → estado difícil correcto.
- [ ] F11. **Módulos:** apagar/encender un módulo de la otra máquina desde su página; razones de «no disponible» correctas.
- [ ] F12. **Ajustes y paleta:** cada acción de Ajustes y cada comando de la paleta hace lo que dice.
- [ ] F13. **Actualizaciones:** buscar versión contra un `latest.json` de prueba, plan de actualizar las dos máquinas (doble); release real queda documentado.
- [ ] F14. **Bandeja:** abrir la app, aplicar escena y ver el estado desde la barra de menú/bandeja.

## 3. Hardware (`[hw]`, suite T-123)

- [ ] H1. Agentes instalados en la PC y en la Mac se emparejan por Wi-Fi.
- [ ] H2. Túnel PC→Mac y Mac→PC con procesos reales.
- [ ] H3. 34" compatible: lectura de estado y cambio de entrada con vuelta automática.
- [ ] H4. Cruce de cursor real con entrada sintética en las dos direcciones.
- [ ] H5. USB-C: pendiente del cable TB4/USB4 (A6); no bloquea FINAL_OK si queda documentado.
