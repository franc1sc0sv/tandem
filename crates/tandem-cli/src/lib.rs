//! `tandem`: the MCP (stdio) and the tunnels CLI (D8, DESIGN §8).
//!
//! Both are clients of the agent's IPC and share everything: the tools table
//! ([`spec`]) describes name, parameters and descriptions once; [`api::Api`] runs
//! them against the IPC; the MCP ([`mcp`]) exposes them over JSON-RPC and the CLI ([`cli`]) reads
//! them from the command line. Neither has tunnel logic of its own.

pub mod api;
pub mod cli;
pub mod mcp;
pub mod render;
pub mod spec;
