//! Tiempos compartidos de las pruebas de dos agentes.
//!
//! Los de producción (ping de 250 ms, conexión de 1 s, latido de 3 s) son demasiado
//! justos para un runner de CI lento con un handshake QUIC en debug: el enlace se
//! daba por caído y los agentes nunca se emparejaban. Aquí se holgan, pero siguen
//! acotados: una caída real se detecta en segundos, muy dentro del `TIMEOUT` de cada prueba.
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
