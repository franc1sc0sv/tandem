# Mockups

- `main.html`, `next.html`: UI de v0.1 y v0.2 (historia).
- **`v03/`: referencia visual e interactiva de v0.3** (fuente de verdad para la UI). Copia de los archivos del diseño publicado (`https://claude.ai/artifact/9iR76c5fczvnpXCbp9BnJt`, privado). Cada `*.dc.html` es una pantalla de 1280×860; `canvas.json` dice el título y la página de cada una.

## Cómo verlos

Los `.dc.html` usan un formato de plantillas (`<x-dc>`, `{{…}}`, `<sc-for>`, `<sc-if>`, clase `Component extends DCLogic`) cuyo runtime original (`support.js`) no está en el repo. `tools/mockup-render` (T-125) trae uno propio:

```sh
cd tools/mockup-render
pnpm install && pnpm exec playwright install chromium
pnpm mockups:serve     # http://127.0.0.1:4173/: índice; los enlaces llevan de una pantalla a otra
pnpm mockups:shot      # capturas → docs/mockups-png/
```

- Las capturas ya generadas están en **`docs/mockups-png/`**: `<mockup>.png` es el estado inicial y `<mockup>--<estado>.png` cada estado interactivo con nombre (por ejemplo `V3-Acomodo--lado-arriba-centro.png`); la lista de estados y sus clics está en `tools/mockup-render/src/states.ts`.
- Comparar una captura de la app con su mockup: `pnpm mockups:compare app.png ../../docs/mockups-png/V3-Puesto.png --diff diff.png` (desde código: `compareWithMockup(captura, { mockup, state, diffPath })` de `@tandem/mockup-render/harness`).
- Si cambias un mockup, vuelve a correr `pnpm mockups:shot` y commitea `docs/mockups-png/`. El job `ui (mockups)` de CI las genera como artefacto y avisa si no coinciden con las commiteadas.

Más detalle en `tools/mockup-render/README.md`.

## Qué pantalla es cuál

| Mockup | Pantalla |
|---|---|
| `V3-Puesto` | Puesto (inicio) |
| `V3-Computo` | Túneles |
| `V3-Maquina`, `F-MaquinaMac` | Máquina |
| `V3-Monitores`, `F-NuevaEscena` | Monitores y escenas |
| `V3-Acomodo`, `F-PruebaCruce` | Acomodo y prueba de cruce |
| `V3-Red` | Red |
| `F-Asis1`, `V3-Asistente`, `F-Asis3`…`F-Asis6` | Asistente (pasos 1–6) |
| `F-Transacciones`, `F-Avisos` | Progreso de escena/energía, avisos |
| `F-Ajustes`, `F-Paleta` | Ajustes, paleta ⌘K |
| `V3-Estados` | Estados difíciles |
| `V3-Bandeja` | Barra de menú / bandeja |
| `F-ClaudeMCP` | Lo que ve Claude (herramientas MCP) |

## Lo que los mockups dicen y DECISIONS corrige

- **Tercera máquina** (`F-AgregarHost`, la «MacBook Air», «Agregar máquina» en la barra lateral, «Puesto de N máquinas» en las notas): fuera por D1. En su lugar, Ajustes → «Cambiar la otra máquina» (emparejar de nuevo).
- **Tarjeta «Agregar máquina» de `V3-Puesto`** (D1): se tapa en la comparación (200 × 210 px a la derecha de las tarjetas de máquina); en la app ese hueco queda vacío, no hay tarjeta que tapar. Encender / Apagar van dentro de la tarjeta de la otra máquina (que sí se compara), con el estilo de los botones del mockup.
- **Atajo de la paleta** (`⌘K`): los mockups están dibujados en macOS; la app muestra `Ctrl+K` en Windows. El arnés de capturas usa un agente de usuario de macOS.
- **Botón «Instalar» del servicio de administrador** (`F-Asis1`): solo si el módulo de pantallas protegidas (T-027) está disponible; si no, la fila muestra su estado sin botón.
- **«macOS 26 · Apple M5 · 24 GB»** y demás datos de ejemplo: en la app salen del escaneo real (D10).
- **Botones «Simular…»** (borde punteado): son solo del mockup; en la app esos pasos ocurren de verdad.
- **`F-MaquinaMac` sin barra lateral** (G3/D16): es una pantalla de flujo con la ruta «Puesto › MacBook Pro» arriba; la app tiene la barra lateral. Se compara solo el contenido: desde la esquina de su `header` hasta el borde de abajo (en el mockup, `x 32, y 55`, 1216 × 805 px; la app se captura a 1492 px de ancho para que su contenido mida lo mismo). La ruta y la barra quedan fuera de la comparación.
- **Selector de color en la página de esta máquina** (D17): `F-MaquinaMac` no lo dibuja; la app lo tiene en las dos páginas. Región tapada: el `radiogroup` «Color de la máquina» de la cabecera (cuatro círculos de 22 px, 106 × 22 px).
- **Estado de los módulos en `F-MaquinaMac`** (G4): el mockup lo escribe como texto suelto («activo», «en espera», «parcial»); la app usa el chip uniforme (interruptor si se puede apagar). Región tapada: la última celda de cada fila de módulo (`.mod > :last-child`), en la app y en el mockup.
- **«Latencia · 10 min»** (`V3-Red`, gráfica de latencia): el agente guarda 60 s por camino (DESIGN §6, T-102); la app dice «Latencia · 60 s» y el arnés tapa ese rótulo en las dos capturas.
- **`F-Transacciones`: «Dejar de esperar»**: el agente no tiene un comando para cancelar un encendido, así que la app no muestra el botón (misma regla que «olvidar monitor»). Región tapada: el último bloque de la tarjeta de energía en el mockup.
- **`F-Transacciones`: «de la MacBook»**: el mockup nombra la máquina a mano; la app dice el nombre real (D10).
