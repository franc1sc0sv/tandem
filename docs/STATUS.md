# Estado del piloto automático

> Actualizado: 2026-10-15 · Rama de integración: `develop` · `main` no se tocó.

## En una línea

**`develop` sigue igual: solo tiene T-001.** En la ronda 1 se implementaron y revisaron tres tareas (T-002, T-003 y T-061). Las tres tienen el CI en verde y la aprobación del reviewer, pero **ninguna se mergeó**: el clasificador de permisos de Claude Code bloqueó `gh pr merge` con el motivo *"Merge Without Review"*. El piloto se detuvo (FRENO) porque la ronda terminó sin merges.

```
Backlog: 32 tareas
  ✅ en develop          █                                 1   (T-001)
  🟡 PR listo, sin merge ███                               3   (T-002, T-003, T-061)
  ⬜ por hacer (agente)  ████████████████████████          24
  ⛔ necesita al usuario ████                              4   (T-027, T-050, T-051 + T-011/T-012 parcialmente)
```

## ✅ Integrado en `develop`

| Tarea | Título | PR |
|---|---|---|
| T-001 | Esqueleto del workspace con todos los crates | [#1](https://github.com/franc1sc0sv/tandem/pull/1) (mergeado) |

## 🟡 Listo para merge, esperando una acción tuya

| Tarea | Título | PR | CI (ubuntu / windows / macos) | Reviewer | GitHub |
|---|---|---|---|---|---|
| T-061 | CI de pruebas: proptest, cobertura y Windows/macOS | [#2](https://github.com/franc1sc0sv/tandem/pull/2) | ✅ / ✅ / ✅ (+ cobertura ✅) | APPROVE | MERGEABLE, CLEAN |
| T-002 | tandem-proto: mensajes y framing | [#3](https://github.com/franc1sc0sv/tandem/pull/3) | ✅ / ✅ / ✅ | APPROVE | MERGEABLE, CLEAN |
| T-003 | Identidad y emparejamiento | [#4](https://github.com/franc1sc0sv/tandem/pull/4) | ✅ / ✅ / ✅ | APPROVE | MERGEABLE, CLEAN |

**Por qué no se mergearon:** el reviewer aprobó con un *comentario* del PR, no con una review formal de GitHub (`reviewDecision` vacío). El clasificador de modo auto lo interpretó como un merge sin revisión y lo bloqueó. Los agentes no intentaron saltarse el bloqueo.

**Para destrabar, elige una opción:**

1. Mergear a mano, en este orden: `gh pr merge 2 --squash --delete-branch`, después el 3 y después el 4. Después del primer merge, los otros dos van a tener conflicto (ver Riesgos): hay que hacer rebase.
2. Agregar en tu configuración una regla de permiso de Bash para `gh pr merge` y relanzar el piloto. El piloto también hace el rebase.

## ⬜ Qué falta y por qué

### Desbloqueadas en cuanto entren T-002 y T-003

| Tarea | Título | Depende de | Bloqueo |
|---|---|---|---|
| T-004 | Transporte QUIC con autenticación mutua | T-002, T-003 | PRs #3 y #4 sin merge |
| T-007 | tandemd: esqueleto del agente | T-002 | PR #3 sin merge |

### Tareas que se pueden tomar ya (dependen solo de T-001)

| Tarea | Título | needs |
|---|---|---|
| T-010 | DDC/CI en Windows (dxva2) + CLI tandem-ddc | none |
| T-012 | Control de pantallas en macOS | mac-hw |
| T-020 | Modelo de eventos de entrada y tablas de teclas | none |

### Cadena larga (esperan a que avance lo anterior)

| Tarea | Título | Depende de | needs |
|---|---|---|---|
| T-005 | Gestor de enlaces múltiples y política de planos | T-004 | none |
| T-006 | Descubrimiento en la LAN | T-004 | none |
| T-008 | Orquestador: planes ejecutados entre las dos máquinas | T-005, T-007 | none |
| T-011 | Topología de pantallas en Windows | T-010 | windows-hw |
| T-013 | macOS: traer ventanas de la pantalla integrada | T-012 | mac-hw |
| T-014 | MonitorSwitcher y ejecutor de pasos de pantalla | T-008, T-010 | none |
| T-021 | Geometría de pantallas y cruce de bordes | T-020 | none |
| T-022 | Captura de entrada en macOS | T-020 | mac-hw |
| T-023 | Inyección de entrada en macOS | T-020 | mac-hw |
| T-024 | Captura de entrada en Windows | T-020 | windows-hw |
| T-025 | Inyección de entrada en Windows | T-020 | windows-hw |
| T-026 | Sesión de entrada sobre el enlace | T-005, T-020, T-021 | none |
| T-028 | Portapapeles sincronizado | T-026 | none |
| T-029 | Hub USB y roles automáticos | T-026 | none |
| T-030 | Encendido y apagado de la PC | T-008 | none |
| T-040 | Tauri: esqueleto de la app y cliente IPC | T-007 | none |
| T-041 | UI: escena principal | T-040 | none |
| T-042 | UI: barra de menú, bandeja y atajos globales | T-041 | none |
| T-060 | Arnés de simulación del puesto (tandem-sim) | T-008 | none |
| T-062 | Ronda de QA adversarial | T-060 | none |

### ⛔ Necesitan una acción física tuya (`needs: user`, el piloto no las toma)

| Tarea | Título | Qué hace falta |
|---|---|---|
| T-051 | Acceso SSH a la Mac para pruebas | Activar Sesión remota en la Mac y autorizar la llave de WSL (`ssh mac-runner uname -a` debe funcionar). Sin esto no se pueden verificar en hardware T-012, T-013, T-022 y T-023. |
| T-050 | Perfil DDC del 32" y código de vuelta del 34" | Estar frente a los monitores mientras se prueban los códigos de entrada. También necesita T-010. |
| T-027 | Windows: servicio SYSTEM y escritorio seguro | Probar el mouse en un diálogo UAC y decidir el certificado Authenticode. También necesita T-024 y T-025. |

## 🔧 Verificación en hardware pendiente (agregada)

Ninguna tarea integrada ni con PR abierto tiene esta sección todavía: T-001, T-002, T-003 y T-061 son `needs: none`. Esto es lo que va a quedar pendiente en el equipo real cuando se implementen las tareas de hardware, según sus criterios de aceptación:

**PC Windows**
- [ ] T-010 → T-050: comprobar `tandem-ddc` contra el 34" real (`list`, `caps`, `switch ... --return-after`).
- [ ] T-011: spike S3, saber si el canal DDC sobrevive con el monitor desactivado (y si no, I2C por NvAPI).
- [ ] T-011: attach/detach de un monitor con `SetDisplayConfig` (pruebas `#[ignore = "hardware"]`, con los dos monitores por DP).
- [ ] T-024: captura con hooks LL y Raw Input probada en la PC por interop.
- [ ] T-025: inyección con SendInput (teclado, mouse absoluto, rueda) probada en la PC.
- [ ] T-027: mouse funcionando en un diálogo UAC y Ctrl+Alt+Supr.
- [ ] T-050: aislar el código de vuelta a DP del 34" (`0x09` o `0x0F`) y el perfil DDC del 32".

**Mac**
- [ ] T-012: spike S2, quitar y devolver pantallas por API privada (externos y la integrada), con espejo como respaldo.
- [ ] T-013: mover ventanas de la pantalla integrada con Accesibilidad (prueba manual).
- [ ] T-022: CGEventTap, Secure Event Input y permisos TCC (prueba manual).
- [ ] T-023: inyección de teclado, mouse, scroll y teclas multimedia (prueba manual).

Requisito previo para todo lo de la Mac: **T-051** (acceso SSH).

## ⚠️ Riesgos conocidos

1. **Conflictos entre los tres PRs abiertos.** #2, #3 y #4 agregan `proptest` en la misma línea de `[workspace.dependencies]` (con versiones distintas: `1.12.0` en dos y `"1"` en T-003) y cambian `Cargo.lock`. Después del primer merge, los otros dos van a tener conflicto: hay que hacer rebase y regenerar `Cargo.lock`, y conviene unificar la versión en `1.12.0`.
2. **Cada merge lo bloquea el clasificador** mientras no haya una regla de permiso para `gh pr merge` o review formal en GitHub. Así, el piloto nunca puede avanzar más de una ronda.
3. **Vuelta atrás incompleta en el núcleo** (nota de T-061): `Step::inverse` de un `SwitchInput` devuelve `None`, así que si un plan falla, la vuelta atrás no devuelve el monitor a su entrada anterior. Queda para quien endurezca el modelo de estado.
4. **Llave de identidad sin protección real en Windows** (T-003): el archivo solo depende de las ACL del perfil. DPAPI y el Keychain quedan para una tarea posterior que todavía no está en el backlog.
5. **Compatibilidad de protocolo estricta** (T-002): solo se habla con la misma `PROTOCOL_VERSION`, así que las dos máquinas tienen que actualizarse juntas. `postcard` necesita espejos de los tipos del núcleo en `wire.rs`, y un cambio en el núcleo obliga a actualizarlos (lo fuerza el compilador).
6. **Spikes de diseño sin resolver** (DESIGN §11): S2 (API privada de macOS), S3 (DDC con el monitor desactivado), S6 (Wake-on-LAN desde apagado total) y S9 (escritorio seguro). Si alguno sale mal, puede cambiar la arquitectura del monitor fantasma o del encendido remoto.
7. **Los spikes con hardware** (S1, S8) cambian la entrada de los monitores. Toda prueba debe traer vuelta automática, porque podrías quedarte sin ver la PC.
