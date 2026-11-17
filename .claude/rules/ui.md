---
paths:
  - "apps/tandem-ui/**"
---
UI: la referencia visual son los mockups v0.3, `mockups/v03/*.dc.html` (qué pantalla es cuál y lo que DECISIONS corrige: `mockups/README.md`). `mockups/main.html` y `next.html` son historia (v0.1 y v0.2): no se usan como referencia. Si un componente o una pantalla difiere del mockup, se corrige la app: el mockup manda.

Cómo se comprueba:

- `cd apps/tandem-ui && pnpm shots`: captura cada pantalla del modo demo y la compara con la captura de su mockup en `docs/mockups-png/` (similitud y peor cuadro, en `docs/screenshots/v03/similitud.json`), y compara los estilos calculados de cada componente de la galería (`gallery.html`) con el mismo elemento del mockup (`scripts/style-parity-spec.ts`). `pnpm shots --check` falla si una zona con umbral no lo cumple o si una propiedad de estilo difiere; es lo que corre el job `ui (capturas)`.
- Comparar una captura suelta: `cd tools/mockup-render && pnpm mockups:compare app.png ../../docs/mockups-png/V3-Puesto.png --diff diff.png`. Ver un mockup: `pnpm mockups:serve`.

Estilo sobrio: colores apagados, un acento suave por máquina, sin brillos de neón, sin degradados llamativos ni fondos animados. Visual antes que texto. Incluye una captura de pantalla en el PR (las de `docs/screenshots/v03/`, o la galería `galeria.png` si cambian componentes).
