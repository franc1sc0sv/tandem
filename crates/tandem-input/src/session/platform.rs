//! [`Capture`] e [`Injector`] sobre los backends de cada plataforma (T-022 a T-025).
//!
//! Los emuladores son portables (genéricos sobre su sink), así que su adaptación se
//! prueba en Linux con los sinks que registran; las capturas solo existen en su sistema.

#[cfg(any(windows, target_os = "macos"))]
use super::Capture;
use super::Injector;
use crate::event::InputEvent;
use crate::macos::emulate::{self as mac_emulate, MacEmulator, MacSink};
use crate::windows::emulate::{self as win_emulate, Emulator, InputSink};

impl<S: MacSink + Send> Injector for MacEmulator<S> {
    type Error = mac_emulate::EmulateError;

    fn inject(&mut self, event: &InputEvent) -> Result<(), Self::Error> {
        self.emit(event)
    }

    fn refresh(&mut self) -> Result<(), Self::Error> {
        MacEmulator::refresh(self)
    }
}

impl<S: InputSink + Send> Injector for Emulator<S> {
    type Error = win_emulate::EmulateError;

    fn inject(&mut self, event: &InputEvent) -> Result<(), Self::Error> {
        self.emulate(event)
    }

    fn refresh(&mut self) -> Result<(), Self::Error> {
        self.refresh_desktop()
    }
}

#[cfg(target_os = "macos")]
impl Capture for crate::macos::capture::MacCapture {
    type Error = crate::macos::capture::CaptureError;

    fn set_grab(&mut self, grab: bool) -> Result<(), Self::Error> {
        crate::macos::capture::MacCapture::set_grab(self, grab)
    }

    fn warp(&mut self, pos: crate::geometry::Point) -> Result<(), Self::Error> {
        crate::macos::capture::MacCapture::warp(self, pos)
    }
}

#[cfg(windows)]
impl Capture for crate::windows::capture::WindowsCapture {
    type Error = crate::windows::capture::CaptureError;

    fn set_grab(&mut self, grab: bool) -> Result<(), Self::Error> {
        crate::windows::capture::WindowsCapture::set_grab(self, grab);
        Ok(())
    }

    fn warp(&mut self, pos: crate::geometry::Point) -> Result<(), Self::Error> {
        crate::windows::capture::WindowsCapture::warp(self, pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{HidUsage, KeyState};
    use crate::macos::emulate::RecordingSink;

    #[test]
    fn el_emulador_de_mac_inyecta_como_injector() {
        let mut e = MacEmulator::new(RecordingSink::new()).unwrap();
        let key = InputEvent::Key {
            usage: HidUsage::key(0x04),
            state: KeyState::Pressed,
        };
        Injector::inject(&mut e, &key).unwrap();
        assert_eq!(e.sink_mut().take().len(), 1);
        Injector::refresh(&mut e).unwrap();
    }
}
