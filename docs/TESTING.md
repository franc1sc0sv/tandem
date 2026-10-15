# Estrategia de pruebas

Objetivo: cuando se conecte el hardware real, que lo único pendiente de verificar sea lo que **físicamente** no se puede simular.

## Capas

| Capa | Qué | Dónde corre |
|---|---|---|
| Unitarias | Lógica pura: planes, transacciones, geometría, mapa de teclas, protocolo, política de enlaces | Linux / CI x3 |
| Propiedades (`proptest`) | Invariantes: el framing decodifica cualquier partición; la vuelta atrás restaura el estado; ida y vuelta de teclas HID↔Mac↔Win; ningún modo deja a la Mac sin pantalla | Linux / CI x3 |
| Integración | Dos `tandemd` en el mismo proceso por loopback (QUIC real) con backends simulados de pantalla, entrada, energía y periféricos | Linux / CI x3 |
| Simulación del puesto (`tandem-sim`) | Escenarios de punta a punta: modos, caída de enlaces, cambio de hub, apagado, fallos inyectados en cada paso | Linux / CI x3 |
| Plataforma | Código real de Windows y macOS: compila y sus pruebas sin dispositivo corren en los runners | CI windows / macos |
| Hardware | `#[ignore = "hardware"]` y la lista `## Verificación en hardware pendiente` de cada tarea | Equipo real, con el usuario |

## Reglas

- Cada backend de plataforma tiene un doble de prueba que implementa el mismo trait.
- Los fallos se inyectan con un `FaultPlan`: fallar el paso N, timeout, caída de enlace en el instante T.
- Los relojes se controlan con `tokio::time::pause()`; ninguna prueba usa `sleep` real.
- Un bug encontrado = primero una prueba que falla, después el arreglo.
- Cobertura con `cargo llvm-cov` en CI (informativa; objetivo ≥ 80 % en los crates portables).
