//! Shared timings for the two-agent tests.
//!
//! The production ones (2 × 200 ms ping windows, 1 s connect, 3 s heartbeat) are too tight for a slow
//! CI runner with a debug-build QUIC handshake: the link was declared down and the agents
//! never paired. They are relaxed here but still bounded: a real drop is detected in
//! seconds, well within each test's `TIMEOUT`.
#![allow(dead_code)]

use std::time::Duration;

use tandem_input::session::SessionConfig;
use tandem_link::manager::HealthConfig;

pub fn health() -> HealthConfig {
    HealthConfig {
        ping_timeout: Duration::from_secs(2),
        connect_timeout: Duration::from_secs(5),
        ..HealthConfig::default()
    }
}

pub fn session() -> SessionConfig {
    SessionConfig {
        heartbeat_interval: Duration::from_millis(100),
        heartbeat_timeout: Duration::from_secs(6),
        ..SessionConfig::default()
    }
}
