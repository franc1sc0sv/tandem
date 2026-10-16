use proptest::prelude::*;

use super::fake::{Call, FakeWindows, window};
use super::*;
use crate::quartz::fake::{builtin, external};

const B: DisplayId = DisplayId(1);
const D34: DisplayId = DisplayId(2);
const D32: DisplayId = DisplayId(3);

fn bounds(x: i32, y: i32, width: i32, height: i32) -> Bounds {
    Bounds {
        x,
        y,
        width,
        height,
    }
}

/// El puesto: el 34" es la principal en (0, 0), el 32" a su derecha y la integrada
/// abajo a la izquierda.
fn desk() -> Vec<CgDisplay> {
    let mut b = builtin();
    b.main = false;
    b.bounds = bounds(-1512, 1440, 1512, 982);
    let mut m34 = external(2, "LC34G55T");
    m34.main = true;
    m34.bounds = bounds(0, 0, 3440, 1440);
    let mut m32 = external(3, "Odyssey G5");
    m32.bounds = bounds(3440, 0, 2560, 1440);
    vec![b, m34, m32]
}

fn display(displays: &mut [CgDisplay], id: DisplayId) -> &mut CgDisplay {
    displays.iter_mut().find(|d| d.id == id).unwrap()
}

#[test]
fn moves_builtin_windows_to_main_external() {
    let displays = desk();
    let mut api = FakeWindows::new(vec![
        window(0, "Safari", -1400, 1500, 800, 600),
        window(1, "Notas", 100, 100, 600, 400),
        window(2, "Terminal", 3500, 10, 700, 500),
    ]);
    let r = gather(&mut api, &displays).unwrap();
    assert_eq!(r.target, Some(D34));
    assert!(r.complete());
    assert_eq!(r.moved.len(), 1);
    assert_eq!(r.moved[0].window.app, "Safari");
    let to = r.moved[0].to;
    assert_eq!((to.width, to.height), (800, 600));
    assert!(contains(bounds(0, 0, 3440, 1440), to), "{to:?}");
    // Solo se movió la de la integrada, y sin cambiar su tamaño.
    assert_eq!(api.calls, [Call::Move(WindowId(0), to.x, to.y)]);
    assert_eq!(
        api.get(WindowId(1)).unwrap().frame,
        bounds(100, 100, 600, 400)
    );
    assert_eq!(
        api.get(WindowId(2)).unwrap().frame,
        bounds(3500, 10, 700, 500)
    );
}

#[test]
fn keeps_relative_position() {
    let displays = desk();
    // Esquina inferior derecha de la integrada: 1512-512 = 1000 libres en x, 982-482 = 500 en y.
    let w = window(0, "Safari", -1512 + 1000, 1440 + 500, 512, 482);
    let p = plan(&displays, &[w]).unwrap();
    assert_eq!(p.moves[0].to, bounds(3440 - 512, 1440 - 482, 512, 482));

    // Esquina superior izquierda.
    let w = window(0, "Safari", -1512, 1440, 512, 482);
    let p = plan(&displays, &[w]).unwrap();
    assert_eq!(p.moves[0].to, bounds(0, 0, 512, 482));

    // Centrada.
    let w = window(0, "Safari", -1512 + 500, 1440 + 250, 512, 482);
    let p = plan(&displays, &[w]).unwrap();
    assert_eq!(p.moves[0].to, bounds(1464, 479, 512, 482));
}

#[test]
fn shrinks_windows_larger_than_target() {
    let mut displays = desk();
    display(&mut displays, D34).bounds = bounds(0, 0, 1280, 720);
    let mut api = FakeWindows::new(vec![window(0, "Xcode", -1512, 1440, 1512, 982)]);
    let r = gather(&mut api, &displays).unwrap();
    assert_eq!(r.moved[0].to, bounds(0, 0, 1280, 720));
    // Se mueve antes de encoger.
    assert_eq!(
        api.calls,
        [
            Call::Move(WindowId(0), 0, 0),
            Call::Resize(WindowId(0), 1280, 720)
        ]
    );
    assert_eq!(api.get(WindowId(0)).unwrap().frame, bounds(0, 0, 1280, 720));
}

#[test]
fn window_belongs_to_display_with_largest_overlap() {
    let displays = desk();
    // Cruza el borde entre la integrada y el 34": más área en la integrada.
    let mostly_builtin = window(0, "A", -300, 1300, 600, 400);
    // Igual, pero más área en el 34".
    let mostly_main = window(1, "B", -100, 1100, 600, 400);
    // Fuera de todas las pantallas.
    let lost = window(2, "C", -5000, -5000, 100, 100);
    let p = plan(&displays, &[mostly_builtin, mostly_main, lost]).unwrap();
    let ids: Vec<_> = p.moves.iter().map(|m| m.window.id).collect();
    assert_eq!(ids, [WindowId(0)]);
}

#[test]
fn skips_minimized_windows() {
    let displays = desk();
    let mut min = window(0, "Mail", -1400, 1500, 800, 600);
    min.minimized = true;
    let mut api = FakeWindows::new(vec![min, window(1, "Safari", -1400, 1500, 800, 600)]);
    let r = gather(&mut api, &displays).unwrap();
    assert_eq!(r.skipped.len(), 1);
    assert_eq!(r.skipped[0].id, WindowId(0));
    assert_eq!(r.moved.len(), 1);
    assert!(
        api.calls
            .iter()
            .all(|c| !matches!(c, Call::Move(WindowId(0), ..)))
    );
}

#[test]
fn target_is_first_external_when_builtin_is_main() {
    let mut displays = desk();
    display(&mut displays, D34).main = false;
    display(&mut displays, B).main = true;
    let p = plan(&displays, &[window(0, "A", -1400, 1500, 800, 600)]).unwrap();
    assert_eq!(p.target, Some(D34));

    // El primero activo y que no sea espejo.
    display(&mut displays, D34).mirror_of = Some(D32);
    let p = plan(&displays, &[window(0, "A", -1400, 1500, 800, 600)]).unwrap();
    assert_eq!(p.target, Some(D32));
    assert!(contains(bounds(3440, 0, 2560, 1440), p.moves[0].to));
}

#[test]
fn nothing_to_do_when_builtin_off_or_mirrored() {
    let w = [window(0, "A", -1400, 1500, 800, 600)];
    let mut displays = desk();
    display(&mut displays, B).enabled = false;
    assert_eq!(plan(&displays, &w).unwrap(), Plan::default());

    let mut displays = desk();
    display(&mut displays, B).mirror_of = Some(D34);
    assert_eq!(plan(&displays, &w).unwrap(), Plan::default());
}

#[test]
fn nothing_to_do_without_windows_on_builtin_even_without_target() {
    let displays = vec![builtin()];
    let w = [window(0, "A", -5000, 0, 100, 100)];
    assert_eq!(plan(&displays, &w).unwrap(), Plan::default());
}

#[test]
fn errors_without_builtin_or_target() {
    let w = [window(0, "A", 10, 10, 100, 100)];
    let only_external = vec![external(2, "LC34G55T")];
    assert_eq!(plan(&only_external, &w), Err(GatherError::NoBuiltin));
    assert_eq!(plan(&[builtin()], &w), Err(GatherError::NoTarget));

    // El externo está apagado (fantasma quitado): tampoco cuenta.
    let mut displays = desk();
    display(&mut displays, D34).enabled = false;
    display(&mut displays, D32).enabled = false;
    let w = [window(0, "A", -1400, 1500, 800, 600)];
    assert_eq!(plan(&displays, &w), Err(GatherError::NoTarget));
}

#[test]
fn continues_after_a_failing_window() {
    let displays = desk();
    let mut api = FakeWindows::new(vec![
        window(0, "Colgada", -1400, 1500, 800, 600),
        window(1, "Safari", -1400, 1500, 800, 600),
    ]);
    api.stuck.insert(WindowId(0));
    let r = gather(&mut api, &displays).unwrap();
    assert!(!r.complete());
    assert_eq!(r.failed.len(), 1);
    assert_eq!(r.failed[0].0.id, WindowId(0));
    assert!(matches!(r.failed[0].1, GatherError::Os { .. }));
    assert_eq!(r.moved.len(), 1);
    assert_eq!(r.moved[0].window.id, WindowId(1));
}

#[test]
fn without_permission_fails_early() {
    let displays = desk();
    let mut api = FakeWindows::new(vec![window(0, "A", -1400, 1500, 800, 600)]);
    api.trusted = false;
    assert_eq!(
        gather(&mut api, &displays),
        Err(GatherError::PermissionDenied)
    );
    assert!(api.calls.is_empty());
}

#[test]
fn display_of_window_names_app_and_title() {
    let w = window(3, "Safari", 1, 2, 30, 40);
    assert_eq!(w.to_string(), "Safari «Safari 3» 30x40 en (1, 2)");
    let mut untitled = w;
    untitled.title = None;
    assert_eq!(untitled.to_string(), "Safari 30x40 en (1, 2)");
}

fn contains(outer: Bounds, inner: Bounds) -> bool {
    inner.x >= outer.x
        && inner.y >= outer.y
        && inner.x + inner.width <= outer.x + outer.width
        && inner.y + inner.height <= outer.y + outer.height
}

fn arb_window(id: usize) -> impl Strategy<Value = Window> {
    (
        -3000i32..7000,
        -1000i32..3000,
        1i32..4000,
        1i32..2500,
        any::<bool>(),
    )
        .prop_map(move |(x, y, w, h, minimized)| {
            let mut win = window(id, "app", x, y, w, h);
            win.minimized = minimized;
            win
        })
}

fn arb_windows() -> impl Strategy<Value = Vec<Window>> {
    (0usize..12).prop_flat_map(|n| (0..n).map(arb_window).collect::<Vec<_>>())
}

proptest! {
    /// Lo movido cabe entero en el destino, nunca crece y solo sale de la integrada.
    #[test]
    fn moves_fit_target_and_never_grow(
        windows in arb_windows(),
        tw in 640i32..6000,
        th in 480i32..3000,
    ) {
        let mut displays = desk();
        display(&mut displays, D34).bounds = bounds(0, 0, tw, th);
        display(&mut displays, D32).bounds = bounds(tw, 0, 2560, 1440);
        let screens: Vec<&CgDisplay> = displays.iter().collect();
        let p = plan(&displays, &windows).unwrap();
        for m in &p.moves {
            prop_assert!(contains(bounds(0, 0, tw, th), m.to), "{:?}", m);
            prop_assert!(m.to.width <= m.window.frame.width);
            prop_assert!(m.to.height <= m.window.frame.height);
            prop_assert!(!m.window.minimized);
            prop_assert_eq!(screen_of(&screens, m.window.frame), Some(B));
        }
        // Toda ventana visible de la integrada se planea; las demás no.
        let expected = windows
            .iter()
            .filter(|w| !w.minimized && screen_of(&screens, w.frame) == Some(B))
            .count();
        prop_assert_eq!(p.moves.len(), expected);
    }

    /// Tras aplicar el plan, ya no queda ninguna ventana visible en la integrada.
    #[test]
    fn gather_leaves_builtin_empty(windows in arb_windows()) {
        let displays = desk();
        let screens: Vec<&CgDisplay> = displays.iter().collect();
        let mut api = FakeWindows::new(windows);
        let r = gather(&mut api, &displays).unwrap();
        prop_assert!(r.complete());
        for w in api.windows.iter().filter(|w| !w.minimized) {
            prop_assert_ne!(screen_of(&screens, w.frame), Some(B), "{}", w);
        }
    }
}
