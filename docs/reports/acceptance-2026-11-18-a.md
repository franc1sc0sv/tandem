# Aceptación de Tandem v0.3, parte A (2026-11-18)

Parte A de 3: paridad visual y animaciones de todas las pantallas. Base: `origin/develop` en `04f2967`, worktree detached. Quedan fuera la parte B (flujos con agentes reales, F1-F14, `F-ClaudeMCP`) y la C (calidad base, CI, G10, G11).

**Resultado: `ACCEPTANCE: FAIL 8`** (tareas T-140 a T-147). Esta parte no declara terminada la app.

## Cómo se midió

- `pnpm shots --check` (66 capturas y la paridad de estilos de 22 componentes) sobre `dist-demo`, con los umbrales de ACCEPTANCE: similitud ≥ 0.99 y peor cuadro ≥ 0.97. Salida completa: [`pnpm-shots.log`](acceptance/2026-11-18-a/pnpm-shots.log). El comando termina en 0; yo volví a aplicar los umbrales a cada medida (pantalla y zonas) del `similitud.json` y solo quedan por debajo las cinco capturas que el arnés mide sin umbral (tabla de abajo).
- `pnpm mockups:shot` de nuevo: las 72 capturas de `docs/mockups-png/` salen idénticas a las commiteadas (referencias vigentes).
- Capturas temporales (no commiteadas) de los estados del mockup que `shots-spec.ts` no mide: ver T-144 y T-145.
- Máscaras: para cada `ignore` y `content.masks` de `shots-spec.ts` se leyó qué hay debajo en el mockup, su área y su razón: [`mascaras.txt`](acceptance/2026-11-18-a/mascaras.txt).
- Animaciones: en cada una de las 66 capturas (mockup y app, con los mismos clics) se recogieron las animaciones y transiciones calculadas (nombre, duración, iteraciones, curva, propiedades del `@keyframes`) más las que corren (`document.getAnimations()` en cada cuadro); la app sin `still`. Comparación por firma: [`animaciones-faltantes.txt`](acceptance/2026-11-18-a/animaciones-faltantes.txt). Con `prefers-reduced-motion: reduce` se repitió en las 66: cero animaciones, cero transiciones, cero animaciones en marcha, cero errores de consola.
- Reglas visuales: 66 casos del demo (los tres pares mac-win, win-win y mac-mac en las once rutas, los cuatro `trouble`, las cuatro pestañas de Ajustes, los seis pasos del asistente, progreso en curso/éxito/fallo, paleta, avisos, bandeja, túneles vacío y con 12 procesos) con volcado de texto, botones, chips, líneas sobre tarjetas, campos, escalas y errores; tabulación por teclado (foco visible); y 187 clics en botones para detectar los que no cambian nada.
- Evidencia de las fallas: `docs/reports/acceptance/2026-11-18-a/<caso>/` (captura de la app, y mockup y diff cuando hay mockup).

## Reglas globales

| Ítem | Resultado | Detalle |
|---|---|---|
| G1 Solo dos máquinas | FALLA | «Agregar máquina» / «Agrega otra máquina» en la tarjeta «Primera vez» de Estados (T-142). Sin tercera máquina en barra lateral, Puesto, Ajustes, Paleta ni Asistente. |
| G2 Nada fijo a Mac/PC | FALLA | Con dos Windows los túneles dicen «PC → PC» (T-141). Nombres, íconos, colores y escritorios a escala salen de cada máquina en win-win y mac-mac (px por pulgada iguales en cada escena: 9.6 y 8.96). |
| G3 Barra lateral | PASA | Puesto, Túneles con contador, Monitores, Acomodo, Red, una entrada por máquina y Ajustes en las 11 rutas y en los tres pares. |
| G4 Botones, íconos, chips, tarjetas | PASA | Todos los `.btn` llevan ícono; foco visible en 113 elementos tabulados sin una excepción; ninguna tarjeta con línea de color arriba; chips de estado uniformes (25 px, 12 px, pastilla; las otras alturas son las de los mockups); paridad de estilos de 22 componentes ✓. Los renglones de lista, escenas y combos sin ícono son los de los mockups. |
| G5 Sin relleno, sin «Teclado aquí» | FALLA | «Wi-Fi · sin cable» (T-147). No hay «Teclado aquí», «requiere cable», modos viejos ni X/Y en el acomodo. |
| G6 Todo hace algo (parte A) | PASA | 187 clics: ningún botón sin efecto salvo los de acciones del sistema (abrir carpeta, Ajustes del Sistema, Salir) y los que ya estaban elegidos; el efecto real en el agente es de la parte B. |
| G7 Animaciones | FALLA | En el demo el cursor del Puesto se reinicia cada 1,5 s y no cruza ni destella (T-140). El resto de los `@keyframes` y `transition` de los mockups tiene equivalente (diferencias de duración menores, anotadas en `animaciones-faltantes.txt`). |
| G8 `prefers-reduced-motion` | PASA | Cero animaciones, transiciones ni animaciones en marcha en las 66 capturas. |
| G9 Consola y textos | PASA | Cero errores o avisos de consola en 66 casos; ningún `{{`, `undefined`, `NaN`, `null` ni texto en inglés visible. |
| G10 Configuración compartida | PARTE B | |
| G11 Calidad base | PARTE C | |

## Pantallas

| # | Pantalla | Mockups y estados | Resultado | Tarea |
|---|---|---|---|---|
| P1 | Puesto | `V3-Puesto` 99.99 % / 98.4 %; con escena Solo MacBook y Juego 99.99 % / 98.4 % (medidas aparte) | PASA en píxeles; animación FALLA | T-140, T-144 |
| P2 | Túneles | los 7 estados del mockup 100 % / 100 %; vacío y 12 procesos sin comparación (mismos componentes) | PASA (rótulos del par win-win FALLAN) | T-141 |
| P3 | Máquina | `V3-Maquina` 100 % / 99.5 % (contenido), `F-MaquinaMac` 99.99 % / 98.4 % | PASA (máscara débil) | T-143 |
| P4 | Monitores y escenas | `V3-Monitores` ×2, `F-NuevaEscena` ×3: 100 % / ≥ 98.9 % | PASA | |
| P5 | Acomodo | 4 lados × 3 alineaciones, pantalla elegida, no compatible: 100 % / ≥ 99.8 % | PASA | |
| P6 | Prueba de cruce | esperando, cruzó, tiempo agotado: 100 % / ≥ 99.7 %; borde equivocado sin mockup (mismos componentes, comprobado en el DOM) | PASA | T-144 (texto de ACCEPTANCE) |
| P7 | Red | `V3-Red` 99.98 % / 97.9 % | PASA (texto Wi-Fi FALLA) | T-147 |
| P8 | Asistente | pasos 1 a 6 y variantes ≥ 99.98 % / ≥ 97.4 %; estado «Crear un puesto nuevo» 99.27 % / 83.0 % | FALLA | T-145 |
| P9 | Progreso / energía | 3 estados: 99.88 % / 97.2 % | PASA | |
| P10 | Avisos | 99.97 % / 99.4 % (solo la pila; el resto no se compara ni se documenta) | FALLA | T-146 |
| P11 | Ajustes | 5 pestañas: ≥ 99.99 % / ≥ 99.2 % | PASA (README desactualizado) | T-146 |
| P12 | Paleta | 99.95 % / 97.1 % con ≈ 16 % tapado sin razón fuerte; «sin resultados» 99.96 % / 97.1 %; «filtro» no representa un filtro | PASA con reparo | T-143, T-144 |
| P13 | Estados difíciles | `V3-Estados` 100 % / 99.6 %; los cuatro `trouble` se ven bien en Puesto, Máquina y Red | FALLA (texto «Agregar máquina») | T-142 |
| P14 | Bandeja | 100 % / 100 % con 17,8 % tapado sin razón fuerte | PASA con reparo | T-143 |
| P15 | Lo que ve Claude | no es pantalla de la app; herramientas de `tandem mcp` en la parte B | PARTE B | |

## Revisión de máscaras

Tienen razón fuerte y están en el README: «Agregar máquina» (barra lateral, tarjeta del Puesto, fila de Ajustes, paleta), «Sacar del puesto», «Simular…», «Dejar de esperar», «Enviar por Wi-Fi igual», «Bloquear aquí», «Latencia · 10 min», sesiones MCP, el nombre del puesto como texto, el lápiz de la otra máquina, la frase de la escena con el nombre real, los rectángulos de `.cur` en `F-Asis5` y «Faltan 1». Sin razón fuerte: Paleta, Bandeja, el botón «Abrir ajustes» tapado en `F-MaquinaMac` y el párrafo de `F-Asis5` (T-143). Regiones del mockup que nadie compara ni documenta, y README desactualizado (Atajos): T-146.

## Capturas (todas)

Las comparaciones de esta corrida; el resultado «INFORMATIVA» es una captura que el arnés mide sin umbral porque el mockup no dibuja ese estado.

| Captura | Mockup / estado | Similitud | Peor cuadro | Resultado |
|---|---|---|---|---|
| acomodo | V3-Acomodo | 100.00 % | 99.8 % | PASA |
| acomodo-abajo-centro | V3-Acomodo--lado-abajo-centro | 100.00 % | 99.8 % | PASA |
| acomodo-abajo-fin | V3-Acomodo--lado-abajo-fin | 100.00 % | 99.8 % | PASA |
| acomodo-abajo-inicio | V3-Acomodo--lado-abajo-inicio | 100.00 % | 99.8 % | PASA |
| acomodo-arriba-centro | V3-Acomodo--lado-arriba-centro | 100.00 % | 99.8 % | PASA |
| acomodo-arriba-fin | V3-Acomodo--lado-arriba-fin | 100.00 % | 99.8 % | PASA |
| acomodo-arriba-inicio | V3-Acomodo--lado-arriba-inicio | 100.00 % | 99.8 % | PASA |
| acomodo-derecha-centro | V3-Acomodo--lado-derecha-centro | 100.00 % | 99.8 % | PASA |
| acomodo-derecha-fin | V3-Acomodo--lado-derecha-fin | 100.00 % | 99.8 % | PASA |
| acomodo-derecha-inicio | V3-Acomodo--lado-derecha-inicio | 100.00 % | 99.8 % | PASA |
| acomodo-izquierda-centro | V3-Acomodo--lado-izquierda-centro | 100.00 % | 99.8 % | PASA |
| acomodo-izquierda-fin | V3-Acomodo--lado-izquierda-fin | 100.00 % | 99.8 % | PASA |
| acomodo-izquierda-inicio | V3-Acomodo--lado-izquierda-inicio | 100.00 % | 99.8 % | PASA |
| acomodo-no-compatible | V3-Acomodo--pantalla-no-compatible | 100.00 % | 99.8 % | PASA |
| acomodo-pantalla-mac | V3-Acomodo--pantalla-mac | 100.00 % | 99.8 % | PASA |
| ajustes | F-Ajustes | 100.00 % | 99.7 % | PASA |
| ajustes-actualizaciones | F-Ajustes | 100.00 % | 99.8 % | PASA |
| ajustes-actualizaciones-buscado | F-Ajustes | 100.00 % | 99.8 % | PASA |
| ajustes-atajos | F-Ajustes | 100.00 % | 99.8 % | PASA |
| ajustes-avanzado | F-Ajustes | 99.99 % | 99.2 % | PASA |
| ajustes-teclado | F-Ajustes | 100.00 % | 99.8 % | PASA |
| asistente-acomodo | F-Asis5 | 100.00 % | 100.0 % | PASA |
| asistente-acomodo-cruzo | F-Asis5--cruzo | 100.00 % | 99.9 % | PASA |
| asistente-acomodo-esperando | F-Asis5--esperando | 100.00 % | 100.0 % | PASA |
| asistente-acomodo-otro-borde | F-Asis5--otro-borde | 99.99 % | 97.7 % | PASA |
| asistente-listo | F-Asis6 | 100.00 % | 99.8 % | PASA |
| asistente-maquina | F-Asis1 | 99.99 % | 98.2 % | PASA |
| asistente-maquina-ambar | F-Asis1--ambar | 99.99 % | 98.2 % | PASA |
| asistente-maquina-verde-azulado | F-Asis1--verde-azulado | 99.99 % | 98.2 % | PASA |
| asistente-maquina-violeta | F-Asis1--violeta | 99.98 % | 97.4 % | PASA |
| asistente-permisos | F-Asis3 | 99.98 % | 97.6 % | PASA |
| asistente-permisos-todo | F-Asis3--todo-concedido | 100.00 % | 100.0 % | PASA |
| asistente-permisos-uno | F-Asis3--uno-concedido | 99.99 % | 97.8 % | PASA |
| asistente-puesto | V3-Asistente | 100.00 % | 99.8 % | PASA |
| asistente-teclado | F-Asis4 | 100.00 % | 100.0 % | PASA |
| avisos | F-Avisos | 99.97 % | 99.4 % | PASA |
| bandeja | V3-Bandeja | 100.00 % | 100.0 % | PASA |
| cruce | F-PruebaCruce | 100.00 % | 100.0 % | PASA |
| cruce-cruzo | F-PruebaCruce--cruzo | 100.00 % | 99.7 % | PASA |
| cruce-no-cruza | F-PruebaCruce--no-cruza | 100.00 % | 99.7 % | PASA |
| cruce-otro-borde | F-PruebaCruce--no-cruza | 99.67 % | 84.6 % | INFORMATIVA: sin umbral (el mockup no lo dibuja) |
| escena-muestra-mac | F-NuevaEscena--muestra-mac | 100.00 % | 99.8 % | PASA |
| escena-nueva | F-NuevaEscena | 100.00 % | 99.8 % | PASA |
| escena-sin-cambiar | F-NuevaEscena--sin-cambiar | 100.00 % | 99.8 % | PASA |
| estados | V3-Estados | 100.00 % | 99.6 % | PASA |
| maquina-a | F-MaquinaMac | 99.99 % | 98.4 % | PASA |
| maquina-b | V3-Maquina | 100.00 % | 99.5 % | PASA |
| monitores | V3-Monitores | 100.00 % | 98.9 % | PASA |
| monitores-entrada-mac | V3-Monitores--entrada-mac | 100.00 % | 98.9 % | PASA |
| paleta | F-Paleta | 99.95 % | 97.1 % | PASA |
| progreso-en-curso | F-Transacciones | 99.88 % | 97.2 % | PASA |
| progreso-exito | F-Transacciones--exito | 99.88 % | 97.2 % | PASA |
| progreso-vuelta-atras | F-Transacciones--vuelta-atras | 99.88 % | 97.2 % | PASA |
| puesto | V3-Puesto | 99.99 % | 98.4 % | PASA |
| puesto-mac-mac | V3-Puesto | 98.26 % | 60.3 % | INFORMATIVA: sin umbral (D2) |
| puesto-win-win | V3-Puesto | 96.83 % | 32.6 % | INFORMATIVA: sin umbral (D2) |
| red | V3-Red | 99.98 % | 97.9 % | PASA |
| tuneles | V3-Computo | 100.00 % | 100.0 % | PASA |
| tuneles-cerrado | V3-Computo--tunel-cerrado | 100.00 % | 100.0 % | PASA |
| tuneles-doce-procesos | V3-Computo | 99.25 % | 81.1 % | INFORMATIVA: sin umbral (el mockup no lo dibuja) |
| tuneles-dos-procesos | V3-Computo--tunel-2-procesos | 100.00 % | 100.0 % | PASA |
| tuneles-lora | V3-Computo--tunel-lora | 100.00 % | 100.0 % | PASA |
| tuneles-proceso-dos | V3-Computo--proceso-2 | 100.00 % | 100.0 % | PASA |
| tuneles-proyecto-plegado | V3-Computo--proyecto-plegado | 100.00 % | 100.0 % | PASA |
| tuneles-sin-procesos | V3-Computo--tunel-sin-procesos | 100.00 % | 100.0 % | PASA |
| tuneles-vacio | V3-Computo | 98.18 % | 82.4 % | INFORMATIVA: sin umbral (el mockup no lo dibuja) |

## Tareas creadas

| Tarea | Falla |
|---|---|
| T-140 | Cursor del Puesto en demo se reinicia cada 1,5 s; sin cruce ni destello (G7, P1) |
| T-141 | «PC → PC» con dos máquinas Windows en Túneles (G2) |
| T-142 | «Agregar máquina» en Estados (G1, P13) |
| T-143 | Máscaras sin razón fuerte: Paleta, Bandeja, Máquina, Asistente |
| T-144 | Estados del mockup sin medir; ACCEPTANCE.md contradice los mockups en P6 |
| T-145 | Asistente «Crear un puesto nuevo» vs `V3-Asistente--nuevo` (99.27 % / 83.0 %) |
| T-146 | Avisos: región «Notificaciones del sistema» sin comparar ni documentar; README de máscaras desactualizado |
| T-147 | «Wi-Fi · sin cable» (G5, P7) |

## No se pudo comprobar aquí

- Estado `F-Avisos--agregado` («Agregar aviso» es un control del mockup; la app no tiene ese botón).
- Los flujos, el efecto en los agentes y `[hw]`: parte B. CI de `develop` y calidad base: parte C.
