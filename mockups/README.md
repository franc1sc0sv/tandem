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
- **Botón «Instalar» del servicio de administrador** (`F-Asis1`) (T-131): ya no se tapa; la app lo dibuja como el mockup. Funciona en macOS (LaunchDaemon, diálogo de administrador) y en Windows (tarea elevada con UAC y comprobación de ACL); si no aplica (instalación por usuario, sin `.app`), la fila explica por qué y no ofrece el botón (`?admin=off`). El demo pone el servicio pendiente (`?admin=pending`) para que la fila muestre el mismo spinner que el mockup.
- **«macOS 26 · Apple M5 · 24 GB»** y demás datos de ejemplo: en la app salen del escaneo real (D10).
- **Botones «Simular…»** (borde punteado): son solo del mockup; en la app esos pasos ocurren de verdad. Regiones tapadas (`F-Asis5`, «esperando»): las cajas de «Simular cruce» y «Simular otro borde».
- **`F-MaquinaMac` sin barra lateral** (G3/D16): es una pantalla de flujo con la ruta «Puesto › MacBook Pro» arriba; la app tiene la barra lateral. Se compara solo el contenido: desde la esquina de su `header` hasta el borde de abajo (en el mockup, `x 32, y 55`, 1216 × 805 px; la app se captura a 1492 px de ancho para que su contenido mida lo mismo). La ruta y la barra quedan fuera de la comparación.
- **Selector de color en la página de esta máquina** (D17): `F-MaquinaMac` no lo dibuja; la app lo tiene en las dos páginas. Región tapada: el `radiogroup` «Color de la máquina» de la cabecera (cuatro círculos de 22 px, 106 × 22 px).
- **Estado de los módulos en `F-MaquinaMac`** (G4): el mockup lo escribe como texto suelto («activo», «en espera», «parcial»); la app usa el chip uniforme (interruptor si se puede apagar). Región tapada: el estado de cada fila de módulo (`.mod > .chip` en la app, `.mod > span:last-child` en el mockup); el botón «Abrir ajustes» de la fila «Permisos» sí se compara.
- **«Latencia · 10 min»** (`V3-Red`, gráfica de latencia): el agente guarda 60 s por camino (DESIGN §6, T-102); la app dice «Latencia · 60 s» y el arnés tapa ese rótulo en las dos capturas.
- **`F-Transacciones`: «Dejar de esperar»**: el agente no tiene un comando para cancelar un encendido, así que la app no muestra el botón (misma regla que «olvidar monitor»). Región tapada: el último bloque de la tarjeta de energía en el mockup.
- **`F-Transacciones`: «de la MacBook»**: el mockup nombra la máquina a mano; la app dice el nombre real (D10).
- **`F-Asis5`: la clase `.cur` repetida**: el CSS del mockup declara `.cur` para el cursor del dibujo (14 × 14, absoluto) y el paso actual de la lista lleva la misma clase (`.stl.cur`): sale del flujo, su texto se parte en tres líneas y «Listo» sube 44 px. Es un defecto del mockup; la app dibuja la lista como en las demás pantallas. Regiones tapadas en las cuatro capturas de `F-Asis5`: 140 × 60 px en (10, 138) y 300 × 94 px en (0, 258), la parte de la lista de pasos afectada.
- **`F-Asis5`: «de la MacBook a la PC»**: el mockup nombra las máquinas a mano; la app dice los nombres del puesto (D10). Regiones tapadas: en el párrafo de introducción, solo los nombres del segundo renglón (552 × 20 px en (438, 134); lo demás se compara) y, mientras espera el cruce, el veredicto («Esperando el cruce por el borde derecho de…», 440 × 40 px en (350, 482)).
- **`F-Asis3`: «Faltan 1» y «Reiniciar agente»**: el mockup no concuerda en singular y no ofrece reiniciar el agente tras abrir un panel de macOS (T-090: el permiso se concede al proceso, que debe reiniciarse). La app dice «Falta 1» y ofrece el reinicio. Región tapada en el estado «uno concedido»: el pie de 200 × 40 px en (775, 770).

## Regiones que el arnés tapa en T-120 (Ajustes, Estados, Paleta, Bandeja, Avisos)

Regla: solo se tapa una caja que no puede coincidir (dato real, función que no existe) y nunca un título, texto o botón que la app sí tiene. Cada región va con su razón; la fuente es `apps/tandem-ui/scripts/shots-spec.ts`.

**Estados (`V3-Estados`).** `?demo&estados` siembra los textos de ejemplo del mockup (nombres, cifras, la línea `epoch 9/20 · loss 0.0371`), así que títulos, textos y botones de las nueve tarjetas se comparan sin tapar; la altura de las tarjetas sale del CSS real de la app, sin ajustes en la medición. Región tapada:
- Botón «Enviar por Wi-Fi igual» de la tarjeta «Enlace»: el agente no tiene esa función, y sin función no hay botón.
- Tarjeta «Primera vez» (D1, T-142): se tapan solo su frase y el rótulo de su botón; la app dice «Emparejar la otra máquina» y «…aparece apagado hasta que emparejes la otra máquina» en lugar de «Agregar máquina» / «Agrega otra máquina para usar esto» (no hay tercera máquina). El título, la etiqueta y el ícono se comparan.

**Asistente, botón «Cerrar» (T-156).** Los `F-Asis*` no dibujan ningún botón de cerrar; la app agrega uno solo con ícono «x» (`aria-label="Cerrar"`, 32 px, foco visible) al pie del panel lateral, porque sin él quien usa solo el mouse no puede salir a mitad del asistente (la usabilidad manda). Es una diferencia deliberada y mínima: la similitud de los `F-Asis*` sigue en ≥ 99 % sin máscaras nuevas.

**Asistente, «Crear un puesto nuevo» (`V3-Asistente--nuevo`, T-145).** El mockup muestra el campo «Nombre del puesto»; la app no, porque el nombre no existe en el agente ni viaja en el protocolo y un campo que no guarda nada viola DESIGN §3.8. En su lugar la app muestra el código de emparejamiento (el emparejamiento es simétrico, T-097) y la nota «Después, en la otra máquina…». Se tapa solo el panel de debajo de las dos opciones (`asistente-puesto-nuevo`); las opciones, el título y los botones se comparan.

**Ajustes (`F-Ajustes`).**
- Nombre del puesto: el mockup trae un campo editable y la app muestra el texto, porque el nombre no viaja en el puesto y un campo que no guarda nada viola DESIGN §3.8. Se tapa solo esa caja.
- Botón «Sacar del puesto» (mockup): D1, exactamente dos máquinas. Por lo mismo, el lápiz de la otra máquina cae en el borde derecho de la fila en la app; se tapa solo ese lápiz.
- Fila «Agregar una máquina · Mostrar código» (D1): la app dice «Cambiar la otra máquina · Emparejar de nuevo». Se tapan el rótulo y el botón, no la fila.
- Atajos: sin regiones tapadas. La app tiene los seis atajos y `ajustes-atajos` mide 100 %.
- Avanzado: la cuenta de sesiones del servidor MCP; el agente todavía no las cuenta.

**Paleta (`F-Paleta`).** `?demo&paleta` siembra los datos del mockup (la escena «Trabajo» sobre la Mac, la tercera escena «Solo PC» y un túnel por proyecto, «ia-local» y «tandem»), y el camino de los túneles usa los mismos rótulos que la lista («Mac → PC»), así que escenas, túneles, puntos de color y los renglones «Monitores» y «Red» se comparan sin tapar. Única región tapada: «Agregar máquina…» (D1; la app dice «Cambiar la otra máquina…»). Estados `filtro` («solo») y `sin-resultados` («zzz»): se teclea en el campo y se compara igual. La app, además de los títulos, busca en el nombre del grupo («esc» deja todas las escenas; el mockup solo mira el título) y agrega las páginas de máquina en «Ir a»; por eso el estado `filtro` usa un texto que da el mismo resultado en las dos.

**Bandeja (`V3-Bandeja`).** `?demo&bandeja` siembra los datos del mockup: enlace de 0.4 ms (el Puesto dice 0.3), carga de la Mac al 18 %, «Solo PC» como tercera escena y los túneles en tres proyectos (dos activos, «notas» en reposo). Los rótulos «Mac → PC» salen de `shortLabels`, como en Túneles. Única región tapada: «Bloquear aquí» (sin la acción en la IPC no hay botón; T-112, G6).

**Avisos (`F-Avisos`).** Se compara la pila completa (los tres avisos con sus tonos: advertencia, correcto y Claude), sin regiones tapadas: `?demo&avisos` siembra los textos del mockup, incluido el botón «Enviar por Wi-Fi igual», que en el demo no hace nada (la app real no lo muestra) para que la pila mida lo mismo. Técnica de medición: la app dibuja la pila como una capa fija en la esquina de la ventana y el mockup la apila en el flujo dentro de un cuadro; para que el texto caiga en el mismo píxel, durante la captura se oculta la página real (`.shell`; en el mockup es un esqueleto), se pinta el fondo del cuadro y la pila pasa a posición absoluta. Se recorta desde la esquina del primer aviso en cada imagen. El aviso «Cambiado desde… · Deshacer» (D5) no está en `F-Avisos`: lo cubren las pruebas de la app.

**Regiones del mockup que no se comparan.**
- `F-Avisos` se mide recortando la pila de avisos (390 × 681 px desde el primer aviso). Queda fuera la cabecera «Avisos» con el botón «Agregar aviso» (controles de demostración del mockup) y la columna derecha «Notificaciones del sistema» («Solo para lo que pasa mientras no miras Tandem…», con tres notificaciones, una de ellas «MacBook Air entró a «Mi puesto»», tercera máquina, D1). No se implementa: DECISIONS D14 deja fuera el audio y las notificaciones por ahora, y la app no tiene código de notificaciones del sistema. Si el usuario las decide, será una decisión nueva en DECISIONS y entonces se mide.
- `F-ClaudeMCP` es la vista de Claude, no una pantalla de la app: se prueba con las herramientas de `tandem mcp` (parte B de la aceptación).
- Revisadas las demás capturas (`shots-spec.ts`): el resto de cada mockup entra en alguna captura o región con su razón en las secciones anteriores.
