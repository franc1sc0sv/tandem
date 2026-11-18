/**
 * Estados interactivos con nombre de cada mockup: se parte del estado inicial, se aplican los
 * pasos en orden (esperando a que terminen las animaciones después de cada uno) y se captura
 * `docs/mockups-png/<mockup>--<estado>.png`. Los selectores son de Playwright.
 *
 * Cubre los estados que pide `docs/ACCEPTANCE.md` (§1) hasta donde el mockup los tiene.
 */

export type Step = { click: string } | { fill: string; text: string };

export interface MockupState {
  name: string;
  steps: Step[];
}

const click = (selector: string): Step => ({ click: selector });
const button = (text: string): Step => click(`button:has-text("${text}")`);

// V3-Acomodo: la otra máquina en cada lado × cada alineación (con el 34" seleccionado). Las
// alineaciones son Arriba/Centro/Abajo a los lados y Izquierda/Centro/Derecha arriba y abajo;
// en los dos casos son los tres botones del segmento, en ese orden.
const SIDES = [
  ['arriba', 'Arriba'],
  ['abajo', 'Abajo'],
  ['izquierda', 'Izquierda'],
  ['derecha', 'Derecha'],
] as const;
const ALIGNS = ['inicio', 'centro', 'fin'] as const;
const acomodo: MockupState[] = SIDES.flatMap(([side, label]) =>
  ALIGNS.map((al, i) => ({
    name: `lado-${side}-${al}`,
    steps: [click(`.slot[aria-label="${label}"]`), click(`.seg .sb >> nth=${i}`)],
  })),
);

export const STATES: Record<string, MockupState[]> = {
  'F-AgregarHost': [
    { name: 'llega', steps: [button('Simular que llega')] },
    { name: 'termina', steps: [button('Simular que llega'), button('Simular que termina')] },
  ],
  'F-Ajustes': [
    { name: 'atajos', steps: [click('[role="tab"]:has-text("Atajos")')] },
    { name: 'teclado', steps: [click('[role="tab"]:has-text("Teclado y mouse")')] },
    { name: 'actualizaciones', steps: [click('[role="tab"]:has-text("Actualizaciones")')] },
    {
      name: 'actualizaciones-buscado',
      steps: [click('[role="tab"]:has-text("Actualizaciones")'), button('Buscar ahora')],
    },
    { name: 'avanzado', steps: [click('[role="tab"]:has-text("Avanzado")')] },
  ],
  'F-Asis1': [
    { name: 'violeta', steps: [click('[role="radio"][aria-label="Violeta"]')] },
    { name: 'verde-azulado', steps: [click('[role="radio"][aria-label="Verde azulado"]')] },
    { name: 'ambar', steps: [click('[role="radio"][aria-label="Ámbar"]')] },
  ],
  'F-Asis3': [
    { name: 'uno-concedido', steps: [click('button:has-text("Abrir ajustes") >> nth=0')] },
    {
      name: 'todo-concedido',
      steps: [click('button:has-text("Abrir ajustes") >> nth=0'), click('button:has-text("Abrir ajustes") >> nth=0')],
    },
  ],
  'F-Asis5': [
    { name: 'esperando', steps: [button('Probar cruce')] },
    { name: 'cruzo', steps: [button('Probar cruce'), button('Simular cruce')] },
    { name: 'otro-borde', steps: [button('Probar cruce'), button('Simular otro borde')] },
  ],
  'F-Avisos': [
    { name: 'agregado', steps: [button('Agregar aviso')] },
    { name: 'cerrado', steps: [click('button[aria-label="Cerrar"] >> nth=0')] },
  ],
  'F-NuevaEscena': [
    { name: 'muestra-mac', steps: [click('[role="radio"]:has-text("MacBook Pro") >> nth=0')] },
    // `kopts` (a quién sigue el teclado) se calcula pero la plantilla no lo muestra.
    { name: 'sin-cambiar', steps: [click('[role="radio"]:has-text("Sin cambiar")')] },
  ],
  'F-Paleta': [
    { name: 'filtro', steps: [{ fill: 'input[aria-label="Buscar"]', text: 'solo' }] },
    { name: 'sin-resultados', steps: [{ fill: 'input[aria-label="Buscar"]', text: 'zzz' }] },
  ],
  'F-PruebaCruce': [
    { name: 'cruzo', steps: [button('Simular cruce')] },
    { name: 'no-cruza', steps: [button('Simular que no cruza')] },
  ],
  'F-Transacciones': [
    { name: 'exito', steps: [button('Simular éxito')] },
    { name: 'vuelta-atras', steps: [button('Simular fallo')] },
  ],
  'V3-Acomodo': [
    ...acomodo,
    { name: 'pantalla-mac', steps: [click('.scr:has-text("MacBook Pro")')] },
    { name: 'pantalla-no-compatible', steps: [click('.scr:has-text("Odyssey G5 32")')] },
  ],
  'V3-Asistente': [{ name: 'nuevo', steps: [click('[role="radio"] >> nth=1')] }],
  'V3-Computo': [
    { name: 'proceso-2', steps: [click('[role="option"] >> nth=1')] },
    { name: 'tunel-2-procesos', steps: [click('.tun:has-text("T-101")')] },
    { name: 'tunel-sin-procesos', steps: [click('.tun:has-text("Revisar PR #66")')] },
    { name: 'tunel-lora', steps: [click('.tun:has-text("Entrenar LoRA")')] },
    { name: 'tunel-cerrado', steps: [click('.tun:has-text("ComfyUI")')] },
    { name: 'proyecto-plegado', steps: [click('.proj:has-text("tandem")')] },
  ],
  'V3-Monitores': [{ name: 'entrada-mac', steps: [click('[role="radio"]:has-text("MacBook Pro")')] }],
  'V3-Puesto': [
    { name: 'escena-solo-macbook', steps: [click('.scene:has-text("Solo MacBook")')] },
    { name: 'escena-juego', steps: [click('.scene:has-text("Juego")')] },
  ],
};

export function statesOf(mockup: string): MockupState[] {
  return STATES[mockup] ?? [];
}
