//! `tandem`: el MCP (stdio) y la CLI de los túneles (D8, DESIGN §8).
//!
//! Los dos son clientes de la IPC del agente y comparten todo: la tabla de herramientas
//! ([`spec`]) describe nombre, parámetros y descripciones una sola vez; [`api::Api`] las
//! ejecuta contra la IPC; el MCP ([`mcp`]) las expone por JSON-RPC y la CLI ([`cli`]) las
//! lee de la línea de órdenes. Ninguno de los dos tiene lógica propia de túneles.

pub mod api;
pub mod cli;
pub mod mcp;
pub mod render;
pub mod spec;
