//! Doble de prueba de [`WindowApi`]: ventanas en memoria, fallos programados por
//! ventana y permiso de Accesibilidad que se puede quitar.

use std::collections::BTreeSet;

use super::{GatherError, Window, WindowApi, WindowId};
use crate::quartz::Bounds;

/// Llamada registrada, para comprobar el orden en las pruebas.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    Move(WindowId, i32, i32),
    Resize(WindowId, i32, i32),
}

#[derive(Debug, Clone, Default)]
pub struct FakeWindows {
    pub windows: Vec<Window>,
    /// `false` simula que el usuario no dio permiso de Accesibilidad.
    pub trusted: bool,
    /// Ventanas que rechazan cambios (por ejemplo, de tamaño fijo o de una app colgada).
    pub stuck: BTreeSet<WindowId>,
    pub calls: Vec<Call>,
}

/// Ventana normal con el marco dado.
pub fn window(id: usize, app: &str, x: i32, y: i32, width: i32, height: i32) -> Window {
    Window {
        id: WindowId(id),
        app: app.into(),
        title: Some(format!("{app} {id}")),
        frame: Bounds {
            x,
            y,
            width,
            height,
        },
        minimized: false,
    }
}

impl FakeWindows {
    pub fn new(windows: Vec<Window>) -> Self {
        Self {
            windows,
            trusted: true,
            ..Self::default()
        }
    }

    pub fn get(&self, id: WindowId) -> Option<&Window> {
        self.windows.iter().find(|w| w.id == id)
    }

    fn window_mut(&mut self, id: WindowId) -> Result<&mut Window, GatherError> {
        if !self.trusted {
            return Err(GatherError::PermissionDenied);
        }
        if self.stuck.contains(&id) {
            return Err(GatherError::Os {
                op: "AXUIElementSetAttributeValue",
                code: -25200,
            });
        }
        self.windows
            .iter_mut()
            .find(|w| w.id == id)
            .ok_or(GatherError::UnknownWindow(id))
    }
}

impl WindowApi for FakeWindows {
    fn windows(&mut self) -> Result<Vec<Window>, GatherError> {
        if !self.trusted {
            return Err(GatherError::PermissionDenied);
        }
        Ok(self.windows.clone())
    }

    fn move_to(&mut self, id: WindowId, x: i32, y: i32) -> Result<(), GatherError> {
        self.calls.push(Call::Move(id, x, y));
        let w = self.window_mut(id)?;
        w.frame.x = x;
        w.frame.y = y;
        Ok(())
    }

    fn resize(&mut self, id: WindowId, width: i32, height: i32) -> Result<(), GatherError> {
        self.calls.push(Call::Resize(id, width, height));
        let w = self.window_mut(id)?;
        w.frame.width = width;
        w.frame.height = height;
        Ok(())
    }
}
