# mockup-render

Renderiza los mockups de `mockups/v03/*.dc.html` sin el runtime original, saca capturas a
`docs/mockups-png/` y compara capturas (DESIGN §12: cada pantalla ≥ 99 % parecida a su mockup).

```sh
cd tools/mockup-render
pnpm install
pnpm exec playwright install chromium   # en CI: --with-deps chromium

pnpm mockups:serve                       # http://127.0.0.1:4173/ para verlos y navegarlos
pnpm mockups:shot                        # todas las capturas → docs/mockups-png
pnpm mockups:shot --only V3-Acomodo      # solo un mockup (y sus estados)
pnpm mockups:compare app.png ../../docs/mockups-png/V3-Puesto.png --diff diff.png
pnpm test                                # runtime, comparación y los 23 mockups
```

## Piezas

- **`support.js`**: runtime propio del formato (`<x-dc>`, `<helmet>`, `{{expr}}`, `<sc-for>`,
  `<sc-if>`, `Component extends DCLogic` con `state`, `setState` y `renderVals()`, `data-props`).
  Lee la plantilla como texto inerte (con `<plaintext>` + DOMParser) para que atributos como
  `d="{{…}}"` no den errores de consola, y en cada `setState` parchea el DOM en lugar de
  reemplazarlo, así las transiciones corren y un `<input>` no pierde el foco. Lo que no se puede
  evaluar queda en `window.__dc.errors`, y las expresiones que dan `undefined` en `window.__dc.missing`.
- **`src/states.ts`**: los estados con nombre de cada mockup (pasos de clic o de escritura).
  Cada estado da `docs/mockups-png/<mockup>--<estado>.png`.
- **`src/render.ts`**: abre cada mockup en Chromium headless al tamaño de su `$preview`
  (1280×860, o 380×600 la bandeja), espera las animaciones de entrada, aplica los pasos y captura.
  Las infinitas (pulsos, brillos) se capturan en su estado inicial (`animations: 'disabled'`).
- **`src/compare.ts`**: `compare(app, mockup) → { similarity, worstTile, diffPng, … }` con
  pixelmatch (tolerancia de color 0.1, sin contar los píxeles de antialias). `worstTile` es el
  cuadro de 64×64 con menos parecido: cambiar un chip o un estado suele mover menos del 1 % de
  los píxeles, así que la similitud global sola no lo detecta.
- **`src/browser.ts`**: `installRoutes`, `waitForMockup`, `settle` y `checkPage`, reutilizables
  por el arnés de la app (T-112) y por las pruebas de cada pantalla.

## Capturas deterministas

- Sin red: los mockups, `support.js` y las fuentes se sirven desde Node dentro del navegador.
  Inter y JetBrains Mono salen de `@fontsource/*` (las mismas que pide la hoja de Google Fonts
  de los mockups); cualquier otra petición externa se bloquea y cuenta como error.
- Dos corridas dan PNG idénticos. Las de referencia son las de Chromium en Linux (WSL o el job
  `ui (mockups)` de CI, que además las sube como artefacto `mockups-png` y las compara con las
  commiteadas).
- Si cambia un mockup o un estado: `pnpm mockups:shot` y commitear `docs/mockups-png/`. La
  prueba `docs/mockups-png tiene exactamente una captura por mockup y estado` avisa si sobran
  o faltan.

## Usarlo desde la app

`tandem-ui` no comparte workspace de pnpm con esta carpeta. Para T-112…T-120 se puede depender
de ella con `"@tandem/mockup-render": "link:../../tools/mockup-render"` e importar desde
`@tandem/mockup-render` (`compare`, `settle`, `installRoutes`, `SHOTS_DIR`…).
