//! Suite de pruebas en hardware (T-123).
//!
//! Dos piezas: el orquestador (`scripts/hw/run.sh`) que compila, instala y arranca agentes de
//! prueba en las dos máquinas, y este crate, que corre en cada máquina: aprovisiona la
//! identidad y la config del agente de prueba, hace las comprobaciones hablando con su IPC
//! y escribe un resultado por línea (JSON) que el orquestador junta en el informe.
//!
//! Nada de esto corre en CI: lo que toca dispositivos es `#[ignore = "hardware"]` y el
//! binario solo lo lanza el orquestador.

pub mod checks;
pub mod conn;
pub mod latency;
pub mod monitor;
pub mod provision;
pub mod report;
