//! Which monitor each screen is (T-340): the system gives, for each screen, the identity of
//! its EDID ([`DisplayKey`]), and the agent links it to the monitor it sees over DDC.
//!
//! The platform readers live next to the bounds they share an order with
//! (`macos::emulate::displays`, `windows::emulate::displays`); here is what is portable.

use tandem_core::DisplayKey;

use crate::geometry::Rect;

/// A screen as the system lists it: its bounds and, if the system says, its EDID identity.
pub type Display = (Rect, Option<DisplayKey>);

/// The registry key that holds the EDID of a monitor on Windows, from the device interface
/// name `EnumDisplayDevicesW` gives with `EDD_GET_DEVICE_INTERFACE_NAME`
/// (`\\?\DISPLAY#SAM7105#5&2b2a3f2&0&UID4352#{e6f07b5f-…}`):
/// `SYSTEM\CurrentControlSet\Enum\DISPLAY\SAM7105\5&2b2a3f2&0&UID4352\Device Parameters`
/// under `HKEY_LOCAL_MACHINE`, value `EDID`.
pub fn windows_edid_key(interface: &str) -> Option<String> {
    let rest = interface
        .strip_prefix(r"\\?\")
        .or_else(|| interface.strip_prefix(r"\\.\"))?;
    let mut parts = rest.split('#');
    let (class, hardware, instance) = (parts.next()?, parts.next()?, parts.next()?);
    let ok = |s: &str| !s.is_empty() && !s.contains(['\\', '/']);
    if !class.eq_ignore_ascii_case("DISPLAY") || !ok(hardware) || !ok(instance) {
        return None;
    }
    Some(format!(
        r"SYSTEM\CurrentControlSet\Enum\DISPLAY\{hardware}\{instance}\Device Parameters"
    ))
}

/// The CoreGraphics numbers of a screen as an EDID identity: `CGDisplayVendorNumber`,
/// `CGDisplayModelNumber` and `CGDisplaySerialNumber` are the EDID's manufacturer, product and
/// serial. A vendor that does not fit in 16 bits (`kDisplayVendorIDUnknown`) or is 0 says nothing.
pub fn mac_display_key(vendor: u32, model: u32, serial: u32) -> Option<DisplayKey> {
    let vendor = u16::try_from(vendor).ok().filter(|v| *v != 0)?;
    Some(DisplayKey {
        vendor,
        product: u16::try_from(model).ok()?,
        serial,
    })
}

/// Whether this machine can turn a display off and on (T-341), for the "Monitors" module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisplayControl {
    /// The system does not need it: on Windows the monitor's active input already decides what
    /// the PC owns, so no phantom display is left.
    NotNeeded,
    Available,
    /// It would be needed but the system does not offer it (a macOS without the private call).
    Unavailable(String),
}

/// A screen the system can turn off: its system id (`CGDirectDisplayID` on macOS) and, if the
/// system says, its EDID identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PowerDisplay {
    pub id: u32,
    pub key: Option<DisplayKey>,
}

/// A display the system knows, whether it is on, off or mirroring another (T-353).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SystemDisplay {
    pub id: u32,
    pub key: Option<DisplayKey>,
    /// The machine's own panel (a MacBook's).
    pub builtin: bool,
    /// It shows something: a screen of its own or a mirror of another.
    pub enabled: bool,
    /// The display it mirrors, if it is a mirror.
    pub mirror_of: Option<u32>,
    /// The main display (the one with the menu bar).
    pub main: bool,
}

/// What the built-in display does (T-353), set in one display configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinState {
    Off,
    /// It mirrors the display with this id.
    Mirror(u32),
    /// A screen of its own.
    Own,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DisplayPowerError {
    #[error("display control is not available: {0}")]
    Unavailable(String),
    #[error("{0}")]
    Platform(String),
}

/// Turning a display off and back on (T-341): on macOS, a monitor that shows the other machine
/// would otherwise stay on the desktop as a screen nobody sees.
pub trait DisplayPower: Send + Sync + std::fmt::Debug {
    fn control(&self) -> DisplayControl;
    /// The active screens, in the order the input engine numbers them (`screen-N`).
    fn active(&self) -> Result<Vec<PowerDisplay>, DisplayPowerError>;
    /// Turns the display `id` on or off. A display already in that state is not an error.
    fn set_enabled(&self, id: u32, on: bool) -> Result<(), DisplayPowerError>;
    /// Every display the system knows, on, off or mirroring (T-353). A system that cannot say
    /// lists none, and then the built-in display is never touched.
    fn online(&self) -> Result<Vec<SystemDisplay>, DisplayPowerError> {
        Ok(Vec::new())
    }
    /// The lid of the machine is closed: its built-in display is not to be turned on (T-353).
    fn lid_closed(&self) -> bool {
        false
    }
    /// Puts the built-in display `id` in `state` in one display configuration (T-353).
    fn set_builtin(&self, _id: u32, _state: BuiltinState) -> Result<(), DisplayPowerError> {
        Err(DisplayPowerError::Unavailable(
            "this system does not control a built-in display".into(),
        ))
    }
}

/// A system that does not turn displays off: Windows, and the ones without an engine.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoDisplayPower;

impl DisplayPower for NoDisplayPower {
    fn control(&self) -> DisplayControl {
        DisplayControl::NotNeeded
    }
    fn active(&self) -> Result<Vec<PowerDisplay>, DisplayPowerError> {
        Ok(Vec::new())
    }
    fn set_enabled(&self, _id: u32, _on: bool) -> Result<(), DisplayPowerError> {
        Ok(())
    }
}

/// The EDID of every external display the system has, with or without a DDC/CI channel (T-353):
/// the IOKit registry on macOS; `None` where the DDC bus already lists every display.
pub fn system_edids() -> Option<fn() -> Vec<Vec<u8>>> {
    #[cfg(target_os = "macos")]
    {
        Some(crate::macos::iokit::external_edids)
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// This system's display control: CoreGraphics on macOS, nothing elsewhere.
pub fn system_display_power() -> std::sync::Arc<dyn DisplayPower> {
    #[cfg(target_os = "macos")]
    {
        std::sync::Arc::new(crate::macos::display_power::MacDisplayPower::new())
    }
    #[cfg(not(target_os = "macos"))]
    {
        std::sync::Arc::new(NoDisplayPower)
    }
}

/// A test double: a list of screens that go away when turned off and come back, in their place,
/// when turned on. It records every change and can fail on purpose.
#[derive(Debug)]
pub struct FakeDisplayPower {
    inner: std::sync::Mutex<FakeDisplays>,
}

#[derive(Debug)]
struct FakeDisplays {
    control: DisplayControl,
    /// Every screen with whether it is on.
    all: Vec<(PowerDisplay, bool)>,
    calls: Vec<(u32, bool)>,
    fail: bool,
    /// The built-in display's id, if this machine has one (T-353).
    builtin: Option<u32>,
    /// Which display mirrors which.
    mirrors: std::collections::BTreeMap<u32, u32>,
    /// Displays that are unplugged: not listed at all, and turning them on fails.
    gone: std::collections::BTreeSet<u32>,
    builtin_calls: Vec<(u32, BuiltinState)>,
    /// How many of the next changes fail before they work again.
    fail_next: u32,
    lid_closed: bool,
}

impl FakeDisplayPower {
    /// Screens, all on, in this order.
    pub fn new(displays: impl IntoIterator<Item = PowerDisplay>) -> Self {
        FakeDisplayPower {
            inner: std::sync::Mutex::new(FakeDisplays {
                control: DisplayControl::Available,
                all: displays.into_iter().map(|d| (d, true)).collect(),
                calls: Vec::new(),
                fail: false,
                builtin: None,
                mirrors: std::collections::BTreeMap::new(),
                gone: std::collections::BTreeSet::new(),
                builtin_calls: Vec::new(),
                fail_next: 0,
                lid_closed: false,
            }),
        }
    }

    /// A system without the call: it reports why and changes nothing.
    pub fn unavailable(reason: &str) -> Self {
        let fake = Self::new([]);
        fake.lock().control = DisplayControl::Unavailable(reason.to_owned());
        fake
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, FakeDisplays> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Every `set_enabled` that went through, in order.
    pub fn calls(&self) -> Vec<(u32, bool)> {
        self.lock().calls.clone()
    }

    /// Whether the display `id` is on now.
    pub fn is_on(&self, id: u32) -> bool {
        self.lock().all.iter().any(|(d, on)| d.id == id && *on)
    }

    /// From now on every change fails (or works again).
    pub fn set_failing(&self, fail: bool) {
        self.lock().fail = fail;
    }

    /// The display `id` is the machine's built-in one (T-353).
    pub fn with_builtin(self, id: u32) -> Self {
        self.lock().builtin = Some(id);
        self
    }

    /// Every `set_builtin` that went through, in order.
    pub fn builtin_calls(&self) -> Vec<(u32, BuiltinState)> {
        self.lock().builtin_calls.clone()
    }

    /// The display `id` mirrors this one, if any.
    pub fn mirror_of(&self, id: u32) -> Option<u32> {
        self.lock().mirrors.get(&id).copied()
    }

    /// The display `id` is unplugged (`true`) or plugged back in, in the state it had.
    pub fn unplug(&self, id: u32, gone: bool) {
        let mut s = self.lock();
        if gone {
            s.gone.insert(id);
        } else {
            s.gone.remove(&id);
        }
    }

    /// The display `id` comes back with another id (macOS may give a new one after a
    /// reconnection or a new session).
    pub fn renumber(&self, id: u32, new: u32) {
        let mut s = self.lock();
        for (d, _) in &mut s.all {
            if d.id == id {
                d.id = new;
            }
        }
    }

    /// The lid is closed (`true`) or open.
    pub fn set_lid_closed(&self, closed: bool) {
        self.lock().lid_closed = closed;
    }

    /// The next `n` changes fail, then they work again.
    pub fn fail_next(&self, n: u32) {
        self.lock().fail_next = n;
    }

    /// Turns a display off behind the agent's back (a previous run that crashed).
    pub fn force(&self, id: u32, on: bool) {
        for (d, state) in &mut self.lock().all {
            if d.id == id {
                *state = on;
            }
        }
    }
}

impl DisplayPower for FakeDisplayPower {
    fn control(&self) -> DisplayControl {
        self.lock().control.clone()
    }

    /// As on macOS, a mirror is not an active display (`CGGetActiveDisplayList`).
    fn active(&self) -> Result<Vec<PowerDisplay>, DisplayPowerError> {
        let s = self.lock();
        Ok(s.all
            .iter()
            .filter(|(d, on)| *on && !s.gone.contains(&d.id) && !s.mirrors.contains_key(&d.id))
            .map(|(d, _)| *d)
            .collect())
    }

    fn set_enabled(&self, id: u32, on: bool) -> Result<(), DisplayPowerError> {
        let mut s = self.lock();
        s.check()?;
        if s.gone.contains(&id) {
            return Err(DisplayPowerError::Platform(
                "CGCompleteDisplayConfiguration failed: 1001".into(),
            ));
        }
        let Some((_, state)) = s.all.iter_mut().find(|(d, _)| d.id == id) else {
            return Err(DisplayPowerError::Platform(
                "CGCompleteDisplayConfiguration failed: 1001".into(),
            ));
        };
        *state = on;
        if !on {
            s.mirrors.remove(&id);
        }
        s.calls.push((id, on));
        Ok(())
    }

    fn online(&self) -> Result<Vec<SystemDisplay>, DisplayPowerError> {
        let s = self.lock();
        let main = s
            .all
            .iter()
            .find(|(d, on)| *on && !s.gone.contains(&d.id) && !s.mirrors.contains_key(&d.id))
            .map(|(d, _)| d.id);
        Ok(s.all
            .iter()
            .filter(|(d, _)| !s.gone.contains(&d.id))
            .map(|(d, on)| SystemDisplay {
                id: d.id,
                key: d.key,
                builtin: s.builtin == Some(d.id),
                enabled: *on,
                mirror_of: s.mirrors.get(&d.id).copied(),
                main: main == Some(d.id),
            })
            .collect())
    }

    fn lid_closed(&self) -> bool {
        self.lock().lid_closed
    }

    fn set_builtin(&self, id: u32, state: BuiltinState) -> Result<(), DisplayPowerError> {
        let mut s = self.lock();
        s.check()?;
        if s.builtin != Some(id) {
            return Err(DisplayPowerError::Platform(format!(
                "{id} is not the built-in display"
            )));
        }
        let on = state != BuiltinState::Off;
        if let Some((_, state)) = s.all.iter_mut().find(|(d, _)| d.id == id) {
            *state = on;
        }
        match state {
            BuiltinState::Mirror(of) => {
                s.mirrors.insert(id, of);
            }
            BuiltinState::Off | BuiltinState::Own => {
                s.mirrors.remove(&id);
            }
        }
        s.builtin_calls.push((id, state));
        Ok(())
    }
}

impl FakeDisplays {
    fn check(&mut self) -> Result<(), DisplayPowerError> {
        if let DisplayControl::Unavailable(why) = &self.control {
            return Err(DisplayPowerError::Unavailable(why.clone()));
        }
        if self.fail {
            return Err(DisplayPowerError::Platform("failing on purpose".into()));
        }
        if self.fail_next > 0 {
            self.fail_next -= 1;
            return Err(DisplayPowerError::Platform(
                "CGCompleteDisplayConfiguration failed: 1001".into(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_edid_key_comes_from_the_interface_name() {
        assert_eq!(
            windows_edid_key(
                r"\\?\DISPLAY#SAM7105#5&2b2a3f2&0&UID4352#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}"
            )
            .as_deref(),
            Some(
                r"SYSTEM\CurrentControlSet\Enum\DISPLAY\SAM7105\5&2b2a3f2&0&UID4352\Device Parameters"
            )
        );
        assert_eq!(windows_edid_key(r"\\?\MONITOR#SAM7105#1#{x}"), None);
        assert_eq!(windows_edid_key(r"\\?\DISPLAY#SAM7105"), None);
        assert_eq!(windows_edid_key(r"\\?\DISPLAY#..\..#1#{x}"), None);
        assert_eq!(windows_edid_key("DISPLAY#SAM7105#1#{x}"), None);
    }

    #[test]
    fn the_coregraphics_numbers_are_the_edid_identity() {
        assert_eq!(
            mac_display_key(0x4C2D, 0x7105, 7),
            Some(DisplayKey {
                vendor: 0x4C2D,
                product: 0x7105,
                serial: 7
            })
        );
        assert_eq!(mac_display_key(0x756E_6B6E, 1, 1), None, "unknown vendor");
        assert_eq!(mac_display_key(0, 1, 1), None);
        assert_eq!(mac_display_key(0x610, 0x1_0000, 1), None);
    }

    #[test]
    fn the_double_hides_a_display_while_it_is_off() {
        let d = |id| PowerDisplay { id, key: None };
        let fake = FakeDisplayPower::new([d(1), d(2)]);
        fake.set_enabled(2, false).unwrap();
        assert_eq!(fake.active().unwrap(), [d(1)]);
        fake.set_enabled(2, true).unwrap();
        assert_eq!(fake.active().unwrap(), [d(1), d(2)]);
        assert_eq!(fake.calls(), [(2, false), (2, true)]);
        let none = FakeDisplayPower::unavailable("no symbol");
        assert!(matches!(
            none.set_enabled(1, false),
            Err(DisplayPowerError::Unavailable(_))
        ));
        assert_eq!(NoDisplayPower.control(), DisplayControl::NotNeeded);
    }
}
