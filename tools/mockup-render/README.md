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
  Las infinitas (pulsos, brillos) se quitan para la captura (`animations: 'disabled'`): queda el
  estilo base del elemento.
- **`src/compare.ts`**: `compare(app, mockup) → { similarity, worstTile, diffPng, … }` con
  pixelmatch (tolerancia de color 0.1, sin contar los píxeles de antialias). `worstTile` es el
  cuadro de 64×64 con menos parecido, sobre una rejilla solapada (paso de 32 px, y cuadros
  pegados a los bordes): cambiar un chip o un estado suele mover menos del 1 % de los píxeles,
  así que la similitud global sola no lo detecta, y con una rejilla fija un cambio que cruza
  cuatro cuadros se diluía al ~25 % en cada uno. `region` compara solo una zona.
- **`src/browser.ts`**: `launchBrowser`, `capture`, `settle`, `installRoutes`, `waitForMockup` y
  `checkPage`.
- **`src/harness.ts`**: la API para la app (abajo).

## Capturas deterministas

- Sin red: los mockups, `support.js` y las fuentes se sirven desde Node dentro del navegador.
  Inter y JetBrains Mono salen de `@fontsource/*` (las mismas que pide la hoja de Google Fonts
  de los mockups). Lo que esos subconjuntos no traen (⌃ ⌥ ⌘ → ↵ ≈ ✓ ➜) sale de DejaVu Sans,
  declarada como cara extra de las mismas familias solo para esos bloques; si no, cada máquina
  lo dibuja con su fuente del sistema. Una prueba exige que todo carácter no ASCII que puede
  verse (el texto del HTML, los `content:` del CSS y los textos de `renderVals()`) esté
  cubierto. Cualquier otra petición externa se bloquea y cuenta como error.
- Chromium se abre con `--disable-partial-raster` (`CHROMIUM_ARGS`, `launchBrowser()`). Sin eso
  Chromium vuelve a rasterizar solo la parte invalidada de cada baldosa, y el PNG final depende
  de qué cuadros intermedios llegó a pintar durante una transición: en `V3-Acomodo` la pantalla
  con `opacity: .4` salía con ±1 de color según la corrida (hasta ~10 600 píxeles distintos en
  `lado-arriba-centro`, 12–13 capturas distintas entre dos corridas).
- En una misma máquina, dos corridas dan PNG idénticos bit a bit (la prueba `la misma captura
  sale idéntica bit a bit` lo exige para los estados de `V3-Acomodo` que variaban). Entre
  máquinas no se exige: el job `ui (mockups)` de CI sube sus capturas como artefacto
  `mockups-png` y las compara con las commiteadas con la tolerancia de `compare()` (≥ 0.999).
  Las de referencia son las de Chromium en Linux (WSL).
- Si cambia un mockup o un estado: `pnpm mockups:shot` y commitear `docs/mockups-png/`. La
  prueba `docs/mockups-png tiene exactamente una captura por mockup y estado` avisa si sobran
  o faltan. `mockups:shot` sin `--only` borra las capturas que ya no corresponden a ningún
  mockup o estado, pero solo si no falló nada.

## Usarlo desde la app (T-112…T-120)

`tandem-ui` no comparte workspace de pnpm con esta carpeta. Se depende de ella con
`"@tandem/mockup-render": "link:../../tools/mockup-render"` (y `pnpm install` también en
`tools/mockup-render`, que trae Playwright, pixelmatch y pngjs). El punto de entrada del arnés es
`@tandem/mockup-render/harness`, que no carga Playwright al importarse:

```ts
import { capture, compareWithMockup, launchBrowser } from '@tandem/mockup-render/harness';

const browser = await launchBrowser();              // mismos argumentos que los mockups
const page = await browser.newPage({ viewport: { width: 1280, height: 860 }, deviceScaleFactor: 1 });
await page.goto('http://127.0.0.1:1420/?demo=puesto');
const png = await capture(page, 'docs/screenshots/v03/V3-Puesto.png'); // settle + captura

const r = compareWithMockup(png, {
  mockup: 'V3-Puesto',              // docs/mockups-png/V3-Puesto.png
  state: undefined,                 // o 'escena-juego' → V3-Puesto--escena-juego.png
  diffPath: 'out/V3-Puesto-diff.png',
  region: { x: 0, y: 0, width: 240, height: 860 }, // opcional: solo la barra lateral
});
r.similarity;            // 0–1; DESIGN §12 pide ≥ 0.99
r.worstTile;             // { x, y, width, height, similarity } del peor cuadro de 64×64
r.expectedPath;          // la referencia usada
```

- `capture(page, path?)` espera con `settle` (fuentes y animaciones finitas terminadas) y captura
  con las animaciones infinitas quitadas y sin cursor de texto, igual que los mockups.
- `compareWithMockup` lanza `MissingMockupShotError` si no hay referencia (nombre o estado mal
  escrito) y `SizeMismatchError` si los tamaños no coinciden.
- Las opciones de `compare()` (`threshold`, `includeAA`, `tile`, `region`) también valen.
- `@tandem/mockup-render` (sin `/harness`) exporta todo, incluido `renderMockups`.
