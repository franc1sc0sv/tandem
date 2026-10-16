# Estrategia de pruebas

Objetivo: cuando se conecte el hardware real, que lo único pendiente de verificar sea lo que **físicamente** no se puede simular.

## Capas

| Capa | Qué | Dónde corre |
|---|---|---|
| Unitarias | Lógica pura: planes, transacciones, geometría, mapa de teclas, protocolo, política de enlaces | Linux / CI x3 |
| Propiedades (`proptest`) | Invariantes: el framing decodifica cualquier partición; la vuelta atrás restaura el estado; ida y vuelta de teclas HID↔Mac↔Win; ningún modo deja a la Mac sin pantalla | Linux / CI x3 |
| Integración | Dos `tandemd` en el mismo proceso por loopback (QUIC real) con backends simulados de pantalla, entrada, energía y periféricos | Linux / CI x3 |
| Binario (`tandemd/tests/two_processes.rs`) | Dos procesos `tandemd` compilados con `--sim-backends`, emparejados por archivos y unidos por QUIC en loopback; la prueba es la UI por la IPC real. Modos, estado en los dos y matar un proceso a mitad de un cambio | CI Linux |
| Simulación del puesto (`tandem-sim`) | Escenarios de punta a punta: modos, caída de enlaces, cambio de hub, apagado, fallos inyectados en cada paso | Linux / CI x3 |
| Plataforma | Código real de Windows y macOS: compila y sus pruebas sin dispositivo corren en los runners | CI windows / macos |
| Hardware | `#[ignore = "hardware"]` y la lista `## Verificación en hardware pendiente` de cada tarea | Equipo real, con el usuario |

## Reglas

- Cada backend de plataforma tiene un doble de prueba que implementa el mismo trait.
- Los fallos se inyectan con un `FaultPlan`: fallar el paso N, timeout, caída de enlace en el instante T.
- Los relojes se controlan con `tokio::time::pause()`; ninguna prueba usa `sleep` real.
- Un bug encontrado = primero una prueba que falla, después el arreglo.
- Cobertura con `cargo llvm-cov` en CI (informativa; objetivo ≥ 80 % en los crates portables).

## Dos procesos reales (`--sim-backends`)

`crates/tandemd/tests/two_processes.rs` lanza el binario dos veces, con configs y directorios de datos temporales, identidades y `trust.toml` preprovisionados y un enlace fijo por loopback. La feature `sim-backends` de `tandemd` (la activa su propia `[dev-dependencies]`, así que solo existe al compilar las pruebas) agrega la opción `--sim-backends`: pasos que siempre salen bien, sin mDNS ni motor de entrada. Como cada proceso es opaco, los pasos dejan rastro en el directorio de datos (`sim-steps.jsonl`, una línea JSON por paso) y se frenan mientras exista `sim-hold`: así la prueba mata un proceso con un paso a medias sin depender de tiempos. Usa relojes reales con tiempos cortos en la config (`step_timeout_secs`, `pc_lost_after_secs`). En el sandbox de WSL, que no deja crear sockets Unix, se omite; en CI nunca.

## Simulación del puesto (`tandem-sim`)

`crates/tandem-sim` levanta dos `tandemd` reales en el mismo proceso: cada uno con su orquestador, su gestor de enlaces (`tandem-link`) y el protocolo de control real (`PeerClient` / `peer::serve`). Lo único simulado es lo de afuera:

- **Puesto físico** (`world`): la entrada real de cada monitor (el 34" obedece `0x06`/`0x03` para HDMI y `0x0F`/`0x09` para DP, ignora los demás códigos sin error y al leer `0x60` siempre dice `0x09`), el escritorio de cada máquina, la pantalla integrada, la energía de la PC (Wake-on-LAN, que solo llega por Ethernet; arranque de 45 s; apagado), la geometría de cada motor de entrada y el hub USB.
- **Red** (`net`): un enlace en memoria por USB4, Ethernet y Wi-Fi. Cortar un enlace es silencio, como un cable desenchufado; los flujos abiertos por él no vuelven aunque el cable vuelva. Con la PC apagada su agente no contesta por ningún enlace.
- **Fallos** (`FaultPlan`): en el paso que se quiera de cualquiera de las dos máquinas, `Fail` (falla sin aplicarse), `Hang` (no termina: lo corta el tiempo máximo), `Delay`, `CutLink` (se cae un enlace al empezar el paso) y `ApplyThenCut` (se aplica y la respuesta se pierde con el enlace). También `cut_link_at` para cortar un enlace en un instante. Cada fallo se dispara una vez salvo `.times(n)`, así el inverso del paso no falla también.

Las comprobaciones comparan la verdad física con lo que cree el agente: `Desk::check` (monitores, energía, escritorios sin fantasmas, pantalla integrada y ningún paso imposible, como uno en la PC apagada), `Desk::check_geometry` y `Desk::pc_agrees` (el agente de la PC tiene el mismo estado).

### Agregar un escenario

Los escenarios van en `crates/tandem-sim/tests/scenarios.rs` (o en otro archivo de `tests/`), siempre con el reloj pausado:

```rust
#[tokio::test(start_paused = true)]
async fn mi_escenario() {
    // Todo en la Mac, PC apagada, tres enlaces. `Desk::builder()` cambia enlaces y tiempos.
    let d = Desk::new().await.unwrap();

    // Llegar al punto de partida por la vía normal.
    let run = d.set_mode(Machine::Mac, Mode::SoloPc).await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Committed));

    // Armar los fallos justo antes de lo que se prueba.
    d.arm(FaultPlan::new().on(
        Machine::Pc,
        Step::PowerOffPc,
        FaultAction::Fail("no se apaga".into()),
    ));
    let run = d.shutdown_pc().await.unwrap();
    assert_eq!(run.outcome(), Some(Outcome::RolledBack));

    // El puesto coincide con el estado y la PC está de acuerdo.
    d.check().await.unwrap();
    d.check_geometry().unwrap();
    d.pc_agrees().await.unwrap();
}
```

- Nada de `std::thread::sleep` ni de esperas reales: para dejar pasar el tiempo, `tokio::time::sleep` (con el reloj pausado avanza solo) o `Desk::settle`.
- Para un paso nuevo de `tandem_core::Step`, agregar su efecto en `SimSteps::apply` (`steps.rs`) y lo que haga falta en `World` y en `World::check`.
- Para un backend nuevo (por ejemplo el `MonitorSwitcher` real detrás de un trait), el doble va en el crate del backend; `tandem-sim` solo modela lo físico.
