//! Manual test of injection on macOS (T-023).
//!
//! `cargo run -p tandem-input --example mac_emulate` on the Mac, with the Accessibility
//! permission for the terminal. Open an empty text editor beforehand and leave it
//! in the foreground: after 3 s the example types, moves the mouse and scrolls.
//! What to watch for is in `docs/tasks/T-023.md`.

#[cfg(target_os = "macos")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::thread::sleep;
    use std::time::Duration;

    use tandem_input::macos::emulate::{CgSink, MacEmulator};
    use tandem_input::{Button, HidUsage, InputEvent, KeyState, Scroll};

    let pause = || sleep(Duration::from_millis(30));
    let mut e = MacEmulator::new(CgSink::new()?)?;
    let tap =
        |e: &mut MacEmulator<CgSink>, usage: HidUsage| -> Result<(), Box<dyn std::error::Error>> {
            for state in [KeyState::Pressed, KeyState::Released] {
                e.emit(&InputEvent::Key { usage, state })?;
                pause();
            }
            Ok(())
        };

    println!("3 s to bring a text editor to the foreground…");
    sleep(Duration::from_secs(3));

    // "tandem", then Shift+physical "ñ" (the ; key on ANSI) and a left arrow.
    for id in [0x17, 0x04, 0x11, 0x07, 0x08, 0x10] {
        tap(&mut e, HidUsage::key(id))?;
    }
    e.emit(&InputEvent::Key {
        usage: HidUsage::key(0xE1),
        state: KeyState::Pressed,
    })?;
    tap(&mut e, HidUsage::key(0x33))?;
    e.emit(&InputEvent::Key {
        usage: HidUsage::key(0xE1),
        state: KeyState::Released,
    })?;
    tap(&mut e, HidUsage::key(0x50))?;

    // Repeat: "a" held 10 times.
    for _ in 0..10 {
        e.emit(&InputEvent::Key {
            usage: HidUsage::key(0x04),
            state: KeyState::Pressed,
        })?;
        sleep(Duration::from_millis(40));
    }
    e.emit(&InputEvent::Key {
        usage: HidUsage::key(0x04),
        state: KeyState::Released,
    })?;

    // A 200 px square with the mouse.
    for (dx, dy) in [(1.0, 0.0), (0.0, 1.0), (-1.0, 0.0), (0.0, -1.0)] {
        for _ in 0..100 {
            e.emit(&InputEvent::MotionRelative {
                dx: dx * 2.0,
                dy: dy * 2.0,
            })?;
            sleep(Duration::from_millis(4));
        }
    }

    // Double click on the typed word (selects the word).
    for _ in 0..2 {
        for state in [KeyState::Pressed, KeyState::Released] {
            e.emit(&InputEvent::Button {
                button: Button::Left,
                state,
            })?;
            pause();
        }
    }

    // Scroll: three notches back and three forward; then pixels.
    for dy in [-120, -120, -120, 120, 120, 120] {
        e.emit(&InputEvent::Scroll(Scroll::Wheel {
            dx_v120: 0,
            dy_v120: dy,
        }))?;
        sleep(Duration::from_millis(150));
    }
    for _ in 0..30 {
        e.emit(&InputEvent::Scroll(Scroll::Pixels { dx: 0.0, dy: -4.0 }))?;
        sleep(Duration::from_millis(10));
    }

    // Media: volume up and down (the indicator must appear).
    tap(&mut e, HidUsage::consumer(0xE9))?;
    sleep(Duration::from_millis(300));
    tap(&mut e, HidUsage::consumer(0xEA))?;

    e.release_all()?;
    println!("done");
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("this example only runs on macOS");
}
