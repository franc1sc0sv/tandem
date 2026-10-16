//! Máquina de estados del servidor: la máquina donde está el teclado y el mouse.
//!
//! Recibe todo lo que captura su sistema y decide, con el [`Layout`], cuándo el
//! cursor cruza a la otra máquina. Mientras el foco es remoto lleva la posición
//! del cursor del cliente (es la autoridad: el cliente solo la sigue) y traduce
//! la entrada a mensajes. No hace E/S: devuelve [`ServerOutput`] y el driver
//! ([`super::quic`]) los ejecuta en orden.

use tandem_core::Machine;

use super::msg::{MotionDatagram, StreamMsg};
use crate::event::{HidUsage, InputEvent, KeyState, Pressed, Scroll};
use crate::geometry::{Layout, Motion, Point};

/// Lo que el driver tiene que hacer, en este orden.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ServerOutput {
    Stream(StreamMsg),
    Datagram(MotionDatagram),
    /// Tragar la entrada local y congelar/ocultar el cursor (`true`) o devolverla.
    Grab(bool),
    /// Llevar el cursor local a esta posición (al volver de la otra máquina).
    Warp(Point),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Focus {
    Local,
    /// Posición del cursor del cliente, en sus coordenadas nativas.
    Remote {
        pos: Point,
    },
}

/// Movimiento y scroll acumulados que todavía no salieron.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Pending {
    dx: f64,
    dy: f64,
    scroll: Option<Scroll>,
}

impl Pending {
    fn is_empty(&self) -> bool {
        self.dx == 0.0 && self.dy == 0.0 && self.scroll.is_none()
    }
}

#[derive(Debug, Clone)]
pub struct ServerSession {
    machine: Machine,
    layout: Layout,
    /// Cursor local; mientras el foco es remoto queda congelado aquí.
    local: Point,
    focus: Focus,
    /// Todo lo presionado en el teclado y el mouse físicos, sin importar el foco:
    /// al entrar se mandan los modificadores que el usuario ya tiene apretados.
    pressed: Pressed,
    seq: u64,
    pending: Pending,
    resync_every: u32,
    since_resync: u32,
    /// El cliente recibió por el stream la posición exacta después del último
    /// movimiento.
    synced: bool,
    /// Atajo que bloquea el cursor en la máquina donde está; vacío = sin atajo.
    lock_shortcut: Vec<HidUsage>,
    locked: bool,
}

impl ServerSession {
    /// `resync_every`: cada cuántos datagramas va uno con la posición absoluta
    /// (además del que sale cuando el mouse se detiene). `0` = en todos.
    pub fn new(machine: Machine, layout: Layout, cursor: Point, resync_every: u32) -> Self {
        Self {
            machine,
            layout,
            local: cursor,
            focus: Focus::Local,
            pressed: Pressed::new(),
            seq: 0,
            pending: Pending::default(),
            resync_every,
            since_resync: 0,
            synced: true,
            lock_shortcut: Vec::new(),
            locked: false,
        }
    }

    /// Atajo de teclado (todas las teclas a la vez) que bloquea y desbloquea el
    /// cursor en la máquina donde está. Las teclas siguen su camino normal.
    pub fn with_lock_shortcut(mut self, keys: Vec<HidUsage>) -> Self {
        self.lock_shortcut = keys;
        self
    }

    /// Si el cursor está bloqueado: no cruza a la otra máquina en ningún sentido.
    pub fn is_locked(&self) -> bool {
        self.locked
    }

    pub fn set_locked(&mut self, locked: bool) {
        self.locked = locked;
    }

    /// Con la tecla recién presionada, ¿se completó el atajo?
    fn shortcut_hit(&self, usage: HidUsage) -> bool {
        let canon = canonical_key(usage);
        !self.lock_shortcut.is_empty()
            && self
                .lock_shortcut
                .iter()
                .any(|k| canonical_key(*k) == canon)
            && self.lock_shortcut.iter().all(|k| {
                self.pressed.is_key_pressed(*k)
                    || alias_key(*k).is_some_and(|a| self.pressed.is_key_pressed(a))
            })
    }

    /// Lleva la cuenta de lo presionado y del atajo de bloqueo.
    fn record(&mut self, event: &InputEvent) {
        self.pressed.record(event);
        if let InputEvent::Key {
            usage,
            state: KeyState::Pressed,
        } = *event
            && self.shortcut_hit(usage)
        {
            self.locked = !self.locked;
            tracing::info!(bloqueado = self.locked, "atajo de bloqueo del cursor");
        }
    }

    pub fn machine(&self) -> Machine {
        self.machine
    }

    /// Si el foco está en la otra máquina.
    pub fn is_remote(&self) -> bool {
        matches!(self.focus, Focus::Remote { .. })
    }

    /// Posición del cursor del cliente, si el foco es remoto.
    pub fn remote_cursor(&self) -> Option<Point> {
        match self.focus {
            Focus::Remote { pos } => Some(pos),
            Focus::Local => None,
        }
    }

    pub fn local_cursor(&self) -> Point {
        self.local
    }

    /// Procesa un evento capturado. Los movimientos van en unidades nativas
    /// locales; un `MotionAbsolute` es la posición real del cursor local, que la
    /// captura informa para corregir el seguimiento.
    pub fn handle(&mut self, event: InputEvent) -> Vec<ServerOutput> {
        let mut out = Vec::new();
        match self.focus {
            Focus::Local => self.handle_local(event, &mut out),
            Focus::Remote { pos } => self.handle_remote(event, pos, &mut out),
        }
        out
    }

    /// Pone el estado al día con un evento capturado cuando no había sesión (sin
    /// enlace, o entre una sesión y la siguiente). Solo recuerda lo presionado y
    /// la posición real del cursor local: el movimiento y el scroll viejos se
    /// descartan, porque aplicarlos tarde haría cruzar sin que el usuario lo pida.
    pub fn catch_up(&mut self, event: InputEvent) {
        match event {
            InputEvent::Key { .. } | InputEvent::Button { .. } => self.record(&event),
            InputEvent::MotionAbsolute { x, y } if !self.is_remote() => {
                self.local = Point::new(x, y);
            }
            InputEvent::MotionAbsolute { .. }
            | InputEvent::MotionRelative { .. }
            | InputEvent::Scroll(_)
            | InputEvent::Modifiers(_) => {}
        }
    }

    fn handle_local(&mut self, event: InputEvent, out: &mut Vec<ServerOutput>) {
        match event {
            InputEvent::Key { .. } | InputEvent::Button { .. } => self.record(&event),
            InputEvent::MotionAbsolute { x, y } => self.local = Point::new(x, y),
            InputEvent::MotionRelative { dx, dy } if self.locked => {
                self.local = self.layout.step_within(self.machine, self.local, dx, dy);
            }
            InputEvent::MotionRelative { dx, dy } => {
                match self.layout.step(self.machine, self.local, dx, dy) {
                    Motion::Local(p) => self.local = p,
                    Motion::Cross { to, pos } if to != self.machine => self.enter(pos, out),
                    // No debería pasar: cruzar "a sí mismo" es quedarse.
                    Motion::Cross { pos, .. } => self.local = pos,
                }
            }
            InputEvent::Scroll(_) | InputEvent::Modifiers(_) => {}
        }
    }

    fn handle_remote(&mut self, event: InputEvent, pos: Point, out: &mut Vec<ServerOutput>) {
        match event {
            InputEvent::Key { usage, state } => {
                self.record(&event);
                // Se reenvía siempre: el cliente decide si cambia algo (una tecla
                // que se apretó antes de entrar no está presionada allá).
                out.push(ServerOutput::Stream(StreamMsg::Key { usage, state }));
            }
            InputEvent::Button { button, state } => {
                self.record(&event);
                out.extend(self.flush().map(ServerOutput::Datagram));
                out.push(ServerOutput::Stream(StreamMsg::Button {
                    button,
                    state,
                    seq: self.seq,
                    pos,
                }));
            }
            InputEvent::MotionRelative { dx, dy } => self.move_remote(pos, dx, dy, out),
            InputEvent::Scroll(s) => {
                if let Some(merged) = merge_scroll(self.pending.scroll, s) {
                    self.pending.scroll = Some(merged);
                } else {
                    // Rueda y píxeles no se suman: sale lo anterior primero.
                    out.extend(self.flush().map(ServerOutput::Datagram));
                    self.pending.scroll = Some(s);
                }
            }
            // La posición real del cursor local no cambia mientras está congelado.
            InputEvent::MotionAbsolute { .. } | InputEvent::Modifiers(_) => {}
        }
    }

    fn move_remote(&mut self, pos: Point, dx: f64, dy: f64, out: &mut Vec<ServerOutput>) {
        let remote = self.machine.other();
        // Nativo local → lógico → nativo remoto: la velocidad se siente igual
        // en pantallas de distinto DPI.
        let (lx, ly) = match self.layout.screen_at(self.machine, self.local) {
            Some(s) => s.to_logical(dx, dy),
            None => (dx, dy),
        };
        let (rx, ry) = match self.layout.screen_at(remote, pos) {
            Some(s) => s.to_native(lx, ly),
            None => (lx, ly),
        };
        let motion = if self.locked {
            Motion::Local(self.layout.step_within(remote, pos, rx, ry))
        } else {
            self.layout.step(remote, pos, rx, ry)
        };
        match motion {
            Motion::Local(p) => {
                // El delta que viaja es lógico; se mide sobre lo que de verdad se
                // movió (contra un borde sin cruce, nada).
                let scale = self.layout.screen_at(remote, pos).map_or(1.0, |s| s.scale);
                self.pending.dx += (p.x - pos.x) / scale;
                self.pending.dy += (p.y - pos.y) / scale;
                self.focus = Focus::Remote { pos: p };
            }
            Motion::Cross { to, pos: back } if to == self.machine => self.leave(back, out),
            Motion::Cross { pos: p, .. } => self.focus = Focus::Remote { pos: p },
        }
    }

    fn enter(&mut self, pos: Point, out: &mut Vec<ServerOutput>) {
        self.focus = Focus::Remote { pos };
        self.pending = Pending::default();
        self.since_resync = 0;
        self.synced = true;
        out.push(ServerOutput::Grab(true));
        out.push(ServerOutput::Stream(StreamMsg::Enter {
            seq: self.seq,
            pos,
            modifiers: self.pressed.modifiers(),
        }));
    }

    fn leave(&mut self, back: Point, out: &mut Vec<ServerOutput>) {
        self.focus = Focus::Local;
        // Lo pendiente era para el cliente, que ahora suelta todo.
        self.pending = Pending::default();
        self.local = back;
        out.push(ServerOutput::Stream(StreamMsg::Leave));
        out.push(ServerOutput::Grab(false));
        out.push(ServerOutput::Warp(back));
    }

    /// Datagrama con lo acumulado, si hay algo. El driver lo llama después de
    /// procesar cada tanda de eventos: lo que llegó junto sale junto.
    pub fn flush(&mut self) -> Option<MotionDatagram> {
        let Focus::Remote { pos } = self.focus else {
            return None;
        };
        if self.pending.is_empty() {
            return None;
        }
        let p = std::mem::take(&mut self.pending);
        self.seq += 1;
        self.since_resync += 1;
        let resync = (self.since_resync >= self.resync_every).then(|| {
            self.since_resync = 0;
            pos
        });
        self.synced = false;
        Some(MotionDatagram {
            seq: self.seq,
            dx: p.dx,
            dy: p.dy,
            scroll: p.scroll,
            resync,
        })
    }

    /// Si hubo movimiento desde la última posición exacta que recibió el cliente.
    pub fn needs_resync(&self) -> bool {
        self.is_remote() && !(self.synced && self.pending.is_empty())
    }

    /// Posición exacta por el stream confiable. El driver la pide cuando el mouse
    /// lleva un rato quieto: si se perdieron datagramas (incluso los periódicos con
    /// la posición), el cursor del cliente termina igual donde debe.
    pub fn resync(&mut self) -> Vec<ServerOutput> {
        let Focus::Remote { pos } = self.focus else {
            return Vec::new();
        };
        if self.synced && self.pending.is_empty() {
            return Vec::new();
        }
        // Lo pendiente sale antes, así el `seq` del Sync lo cubre.
        let mut out: Vec<_> = self
            .flush()
            .map(ServerOutput::Datagram)
            .into_iter()
            .collect();
        self.synced = true;
        out.push(ServerOutput::Stream(StreamMsg::Sync { seq: self.seq, pos }));
        out
    }

    /// La geometría cambió (cambio de modo o de pantallas). Si la otra máquina ya
    /// no tiene pantallas visibles, el cursor vuelve.
    pub fn set_layout(&mut self, layout: Layout) -> Vec<ServerOutput> {
        self.layout = layout;
        let mut out = Vec::new();
        if let Some(s) = self.layout.screen_at(self.machine, self.local) {
            self.local = s.rect.clamp(self.local);
        }
        if let Focus::Remote { pos } = self.focus {
            match self.layout.screen_at(self.machine.other(), pos) {
                Some(s) => {
                    let clamped = s.rect.clamp(pos);
                    if clamped != pos {
                        self.focus = Focus::Remote { pos: clamped };
                        self.synced = false;
                    }
                }
                None => {
                    let back = self.local;
                    self.leave(back, &mut out);
                }
            }
        }
        out
    }

    /// El enlace cayó o cambió: el foco vuelve a esta máquina sin avisar al
    /// cliente (ya no hay por dónde); él suelta todo por su cuenta.
    pub fn link_lost(&mut self) -> Vec<ServerOutput> {
        if !self.is_remote() {
            return Vec::new();
        }
        self.focus = Focus::Local;
        self.pending = Pending::default();
        vec![ServerOutput::Grab(false), ServerOutput::Warp(self.local)]
    }
}

/// macOS entrega PrintScreen, ScrollLock y Pause de un teclado de PC como F13, F14
/// y F15 ([`crate::event::keymap`]): para el atajo son la misma tecla.
fn alias_key(usage: HidUsage) -> Option<HidUsage> {
    const PAIRS: [(u16, u16); 3] = [(0x46, 0x68), (0x47, 0x69), (0x48, 0x6A)];
    if usage.page != crate::event::PAGE_KEYBOARD {
        return None;
    }
    PAIRS.iter().find_map(|&(pc, mac)| {
        if usage.id == pc {
            Some(HidUsage::key(mac))
        } else if usage.id == mac {
            Some(HidUsage::key(pc))
        } else {
            None
        }
    })
}

fn canonical_key(usage: HidUsage) -> HidUsage {
    match alias_key(usage) {
        Some(a) if a.id < usage.id => a,
        _ => usage,
    }
}

/// Suma dos scrolls del mismo tipo; `None` si son de tipos distintos.
fn merge_scroll(acc: Option<Scroll>, s: Scroll) -> Option<Scroll> {
    match (acc, s) {
        (None, s) => Some(s),
        (
            Some(Scroll::Wheel { dx_v120, dy_v120 }),
            Scroll::Wheel {
                dx_v120: x,
                dy_v120: y,
            },
        ) => Some(Scroll::Wheel {
            dx_v120: dx_v120.saturating_add(x),
            dy_v120: dy_v120.saturating_add(y),
        }),
        (Some(Scroll::Pixels { dx, dy }), Scroll::Pixels { dx: x, dy: y }) => {
            Some(Scroll::Pixels {
                dx: dx + x,
                dy: dy + y,
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Button, HidUsage, KeyState, ModifierKey, Modifiers};
    use crate::session::testutil::{
        edge_pc, layout_mac_pc_hidpi, layout_pc_mac, layout_pc_mac_small, layout_pc_only,
    };

    fn session() -> ServerSession {
        ServerSession::new(Machine::Pc, layout_pc_mac(), Point::new(100.0, 100.0), 4)
    }

    fn rel(dx: f64, dy: f64) -> InputEvent {
        InputEvent::MotionRelative { dx, dy }
    }

    fn key(id: u16, state: KeyState) -> InputEvent {
        InputEvent::Key {
            usage: HidUsage::key(id),
            state,
        }
    }

    /// Lleva el cursor al borde y cruza; devuelve las salidas del cruce.
    fn cross(s: &mut ServerSession) -> Vec<ServerOutput> {
        s.handle(InputEvent::MotionAbsolute {
            x: edge_pc(),
            y: 720.0,
        });
        s.handle(rel(5.0, 0.0))
    }

    #[test]
    fn local_no_manda_nada() {
        let mut s = session();
        assert!(s.handle(rel(10.0, 5.0)).is_empty());
        assert_eq!(s.local_cursor(), Point::new(110.0, 105.0));
        assert!(s.handle(key(0x04, KeyState::Pressed)).is_empty());
        assert!(
            s.handle(InputEvent::Scroll(Scroll::Pixels { dx: 0.0, dy: 1.0 }))
                .is_empty()
        );
        assert_eq!(s.flush(), None);
        assert!(!s.is_remote());
    }

    #[test]
    fn cruzar_entra_con_los_modificadores_apretados() {
        let mut s = session();
        s.handle(key(0xE1, KeyState::Pressed)); // Shift izquierdo
        s.handle(key(0x04, KeyState::Pressed)); // A: no es modificador
        let out = cross(&mut s);
        assert_eq!(
            out,
            vec![
                ServerOutput::Grab(true),
                ServerOutput::Stream(StreamMsg::Enter {
                    seq: 0,
                    pos: Point::new(3440.0, 540.0),
                    modifiers: Modifiers::from_iter([ModifierKey::ShiftLeft]),
                }),
            ]
        );
        assert!(s.is_remote());
        assert_eq!(s.remote_cursor(), Some(Point::new(3440.0, 540.0)));
    }

    #[test]
    fn el_movimiento_se_coalesce_y_resincroniza_cada_n() {
        let mut s = session();
        cross(&mut s);
        for _ in 0..3 {
            assert!(s.handle(rel(2.0, 1.0)).is_empty());
        }
        let d = s.flush().unwrap();
        assert_eq!((d.seq, d.dx, d.dy, d.resync), (1, 6.0, 3.0, None));
        assert_eq!(s.flush(), None, "nada pendiente");
        for i in 2..=4 {
            s.handle(rel(1.0, 0.0));
            let d = s.flush().unwrap();
            assert_eq!(d.seq, i);
            // El cuarto datagrama lleva la posición absoluta.
            assert_eq!(d.resync.is_some(), i == 4);
        }
        assert_eq!(s.remote_cursor(), Some(Point::new(3449.0, 543.0)));
    }

    #[test]
    fn el_delta_viaja_en_puntos_logicos() {
        // Cliente PC con 150 %, servidor Mac a 1.0.
        let mut s = ServerSession::new(
            Machine::Mac,
            layout_mac_pc_hidpi(),
            Point::new(3439.0, 720.0),
            0,
        );
        let out = s.handle(rel(1.0, 0.0));
        assert!(matches!(
            out[1],
            ServerOutput::Stream(StreamMsg::Enter { .. })
        ));
        let start = s.remote_cursor().unwrap();
        s.handle(rel(10.0, 0.0));
        let d = s.flush().unwrap();
        assert_eq!((d.dx, d.dy), (10.0, 0.0));
        // Pero el cursor del cliente se mueve en sus unidades nativas.
        assert_eq!(d.resync, Some(Point::new(start.x + 15.0, start.y)));
    }

    #[test]
    fn quieto_resincroniza_una_sola_vez() {
        let mut s = session();
        cross(&mut s);
        assert!(!s.needs_resync(), "el Enter ya lleva la posición");
        assert!(s.resync().is_empty());
        s.handle(rel(3.0, 0.0));
        s.flush().unwrap();
        assert!(s.needs_resync());
        let pos = s.remote_cursor().unwrap();
        assert_eq!(
            s.resync(),
            vec![ServerOutput::Stream(StreamMsg::Sync { seq: 1, pos })]
        );
        assert!(s.resync().is_empty());
        // Con movimiento pendiente, sale primero el datagrama.
        s.handle(rel(1.0, 0.0));
        let out = s.resync();
        assert!(matches!(out[0], ServerOutput::Datagram(d) if d.seq == 2));
        assert!(matches!(
            out[1],
            ServerOutput::Stream(StreamMsg::Sync { seq: 2, .. })
        ));
        assert!(s.resync().is_empty());
        assert!(s.link_lost().len() == 2);
        assert!(
            s.resync().is_empty(),
            "sin foco remoto no hay nada que sincronizar"
        );
    }

    #[test]
    fn el_boton_lleva_la_posicion_y_sale_despues_del_movimiento() {
        let mut s = session();
        cross(&mut s);
        s.handle(rel(4.0, 0.0));
        let out = s.handle(InputEvent::Button {
            button: Button::Left,
            state: KeyState::Pressed,
        });
        let pos = s.remote_cursor().unwrap();
        assert_eq!(out.len(), 2);
        assert!(matches!(out[0], ServerOutput::Datagram(d) if d.seq == 1));
        assert_eq!(
            out[1],
            ServerOutput::Stream(StreamMsg::Button {
                button: Button::Left,
                state: KeyState::Pressed,
                seq: 1,
                pos,
            })
        );
    }

    #[test]
    fn scroll_suma_del_mismo_tipo_y_separa_los_distintos() {
        let mut s = session();
        cross(&mut s);
        let wheel = |dy| {
            InputEvent::Scroll(Scroll::Wheel {
                dx_v120: 0,
                dy_v120: dy,
            })
        };
        assert!(s.handle(wheel(120)).is_empty());
        assert!(s.handle(wheel(60)).is_empty());
        let out = s.handle(InputEvent::Scroll(Scroll::Pixels { dx: 1.0, dy: 2.0 }));
        assert_eq!(out.len(), 1);
        let ServerOutput::Datagram(d) = out[0] else {
            panic!("se esperaba datagrama: {out:?}")
        };
        assert_eq!(
            d.scroll,
            Some(Scroll::Wheel {
                dx_v120: 0,
                dy_v120: 180
            })
        );
        assert_eq!(
            s.flush().unwrap().scroll,
            Some(Scroll::Pixels { dx: 1.0, dy: 2.0 })
        );
    }

    #[test]
    fn volver_por_el_borde_sale_y_devuelve_el_cursor() {
        let mut s = session();
        cross(&mut s);
        s.handle(rel(2.0, 0.0));
        let out = s.handle(rel(-10.0, 0.0));
        let back = s.local_cursor();
        assert_eq!(
            out,
            vec![
                ServerOutput::Stream(StreamMsg::Leave),
                ServerOutput::Grab(false),
                ServerOutput::Warp(back),
            ]
        );
        assert!(!s.is_remote());
        assert_eq!(s.flush(), None, "lo pendiente se descarta al salir");
        assert!(back.x < 3440.0);
    }

    #[test]
    fn las_teclas_se_reenvian_mientras_el_foco_es_remoto() {
        let mut s = session();
        cross(&mut s);
        assert_eq!(
            s.handle(key(0x05, KeyState::Pressed)),
            vec![ServerOutput::Stream(StreamMsg::Key {
                usage: HidUsage::key(0x05),
                state: KeyState::Pressed
            })]
        );
        assert!(
            s.handle(InputEvent::MotionAbsolute { x: 0.0, y: 0.0 })
                .is_empty()
        );
    }

    #[test]
    fn ponerse_al_dia_no_cruza_ni_mueve_pero_recuerda_lo_presionado() {
        let mut s = session();
        s.catch_up(key(0xE1, KeyState::Pressed));
        s.catch_up(InputEvent::MotionAbsolute {
            x: edge_pc(),
            y: 720.0,
        });
        // Contra el borde: con `handle` cruzaría.
        s.catch_up(rel(50.0, 0.0));
        s.catch_up(InputEvent::Scroll(Scroll::Pixels { dx: 0.0, dy: 3.0 }));
        assert!(!s.is_remote());
        assert_eq!(s.local_cursor(), Point::new(edge_pc(), 720.0));
        assert_eq!(s.flush(), None);
        // El Shift que sigue apretado entra con el primer cruce de verdad.
        let out = s.handle(rel(5.0, 0.0));
        assert!(matches!(
            out[1],
            ServerOutput::Stream(StreamMsg::Enter { modifiers, .. })
                if modifiers == Modifiers::from_iter([ModifierKey::ShiftLeft])
        ));
        // Con el foco remoto tampoco mueve nada ni manda nada.
        let remote = s.remote_cursor();
        s.catch_up(rel(10.0, 0.0));
        s.catch_up(InputEvent::MotionAbsolute { x: 0.0, y: 0.0 });
        assert_eq!(s.remote_cursor(), remote);
        assert_eq!(s.local_cursor(), Point::new(edge_pc(), 720.0));
        assert_eq!(s.flush(), None);
    }

    #[test]
    fn caida_del_enlace_devuelve_el_foco_local() {
        let mut s = session();
        assert!(s.link_lost().is_empty());
        cross(&mut s);
        let frozen = s.local_cursor();
        s.handle(rel(5.0, 0.0));
        assert_eq!(
            s.link_lost(),
            vec![ServerOutput::Grab(false), ServerOutput::Warp(frozen)]
        );
        assert!(!s.is_remote());
        assert_eq!(s.flush(), None);
    }

    #[test]
    fn si_la_otra_maquina_deja_de_verse_el_cursor_vuelve() {
        let mut s = session();
        cross(&mut s);
        let out = s.set_layout(layout_pc_only());
        assert_eq!(out[0], ServerOutput::Stream(StreamMsg::Leave));
        assert_eq!(out[1], ServerOutput::Grab(false));
        assert!(!s.is_remote());
    }

    #[test]
    fn geometria_nueva_con_el_cursor_fuera_lo_acomoda_y_resincroniza() {
        let mut s = session();
        cross(&mut s);
        // Mismas pantallas: nada que hacer.
        assert!(s.set_layout(layout_pc_mac()).is_empty());
        assert!(!s.needs_resync());
        s.handle(rel(100.0, 400.0));
        s.flush();
        s.resync();
        assert_eq!(s.remote_cursor(), Some(Point::new(3540.0, 940.0)));
        // La pantalla del cliente se achica: el cursor queda dentro.
        assert!(s.set_layout(layout_pc_mac_small()).is_empty());
        assert_eq!(s.remote_cursor(), Some(Point::new(3540.0, 599.0)));
        assert_eq!(
            s.resync(),
            vec![ServerOutput::Stream(StreamMsg::Sync {
                seq: 1,
                pos: Point::new(3540.0, 599.0)
            })]
        );
    }

    #[test]
    fn el_atajo_bloquea_el_cursor_en_su_maquina() {
        // Ctrl izquierdo + ScrollLock.
        let mut s = session().with_lock_shortcut(vec![HidUsage::key(0xE0), HidUsage::key(0x47)]);
        s.handle(InputEvent::MotionAbsolute {
            x: edge_pc(),
            y: 720.0,
        });
        // ScrollLock solo no alcanza.
        s.handle(key(0x47, KeyState::Pressed));
        s.handle(key(0x47, KeyState::Released));
        assert!(!s.is_locked());
        s.handle(key(0xE0, KeyState::Pressed));
        s.handle(key(0x47, KeyState::Pressed));
        assert!(s.is_locked());
        assert!(s.handle(rel(50.0, 0.0)).is_empty(), "bloqueado no cruza");
        assert!(!s.is_remote());
        assert_eq!(s.local_cursor(), Point::new(edge_pc(), 720.0));
        // Repetir el atajo desbloquea y el cursor cruza otra vez.
        s.handle(key(0x47, KeyState::Released));
        s.handle(key(0x47, KeyState::Pressed));
        s.handle(key(0x47, KeyState::Released));
        s.handle(key(0xE0, KeyState::Released));
        assert!(!s.is_locked());
        assert!(matches!(
            s.handle(rel(5.0, 0.0))[0],
            ServerOutput::Grab(true)
        ));
    }

    #[test]
    fn bloqueado_en_la_otra_maquina_no_vuelve() {
        let mut s = session().with_lock_shortcut(vec![HidUsage::key(0x47)]);
        cross(&mut s);
        // El atajo también llega al cliente: es una tecla más.
        let out = s.handle(key(0x47, KeyState::Pressed));
        assert!(matches!(
            out[0],
            ServerOutput::Stream(StreamMsg::Key { .. })
        ));
        assert!(s.is_locked());
        assert!(s.handle(rel(-500.0, 0.0)).is_empty());
        assert!(s.is_remote(), "contra el borde, sin volver");
        assert_eq!(s.remote_cursor().map(|p| p.x), Some(3440.0));
    }

    #[test]
    fn scroll_lock_de_un_teclado_de_pc_en_la_mac_es_f14() {
        let mut s = ServerSession::new(Machine::Mac, layout_pc_mac(), Point::new(3500.0, 10.0), 4)
            .with_lock_shortcut(vec![HidUsage::key(0x47)]);
        s.handle(key(0x69, KeyState::Pressed));
        assert!(s.is_locked());
        assert_eq!(canonical_key(HidUsage::key(0x69)), HidUsage::key(0x47));
        assert_eq!(canonical_key(HidUsage::key(0x04)), HidUsage::key(0x04));
        assert_eq!(alias_key(HidUsage::consumer(0x47)), None);
    }

    #[test]
    fn sin_atajo_nunca_se_bloquea() {
        let mut s = session();
        s.handle(key(0x47, KeyState::Pressed));
        assert!(!s.is_locked());
        s.set_locked(true);
        assert!(s.is_locked());
    }

    #[test]
    fn merge_scroll_satura() {
        let big = Scroll::Wheel {
            dx_v120: i32::MAX,
            dy_v120: 0,
        };
        assert_eq!(merge_scroll(Some(big), big), Some(big));
    }
}
