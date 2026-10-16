//! Doble de prueba del escáner: los dispositivos los pone la prueba.

use std::sync::{Arc, Mutex, PoisonError};

use crate::id::UsbId;
use crate::watcher::DeviceScanner;

#[derive(Debug, Default)]
struct Inner {
    devices: Vec<UsbId>,
    fail: bool,
}

/// Escáner simulado. Se maneja desde la prueba con [`FakeScanner::handle`].
#[derive(Debug, Clone, Default)]
pub struct FakeScanner {
    inner: Arc<Mutex<Inner>>,
}

/// Mando del escáner simulado: conectar y desconectar dispositivos, o hacer que
/// las lecturas fallen.
#[derive(Debug, Clone)]
pub struct FakeDevices {
    inner: Arc<Mutex<Inner>>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("lectura simulada fallida")]
pub struct FakeScanError;

impl FakeScanner {
    pub fn new(devices: Vec<UsbId>) -> Self {
        FakeScanner {
            inner: Arc::new(Mutex::new(Inner {
                devices,
                fail: false,
            })),
        }
    }

    pub fn handle(&self) -> FakeDevices {
        FakeDevices {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl FakeDevices {
    pub fn set(&self, devices: Vec<UsbId>) {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .devices = devices;
    }

    pub fn fail(&self, fail: bool) {
        self.inner
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .fail = fail;
    }
}

impl DeviceScanner for FakeScanner {
    type Error = FakeScanError;

    fn scan(&mut self) -> Result<Vec<UsbId>, FakeScanError> {
        let inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        if inner.fail {
            Err(FakeScanError)
        } else {
            Ok(inner.devices.clone())
        }
    }
}
