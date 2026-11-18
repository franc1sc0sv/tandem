//! El cursor en vivo (T-128): posición y cruces de borde para la UI, sin costo cuando
//! nadie mira.
//!
//! - [`CursorGate`]: un interruptor atómico que el agente enciende mientras haya alguna
//!   suscripción (la UI de esta máquina o la de la otra). Los drivers lo leen en el camino
//!   caliente: apagado, el costo por evento es una lectura relajada y nada más.
//! - [`Limiter`]: deja pasar como mucho [`MAX_HZ`] muestras por segundo. Lo que se frena no
//!   se pierde: queda pendiente y sale en cuanto toca, para que el punto de la UI no se
//!   quede unos píxeles antes de donde el mouse se detuvo.
//! - [`CursorFeed`]: lo que cada driver lleva dentro (la máquina, el límite y por dónde sale).
//!
//! Las muestras las publica la máquina que tiene el foco: el servidor mientras el cursor
//! está en su pantalla, el cliente mientras está en la suya. Los cruces los publica siempre
//! el servidor, que es quien los decide.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tandem_core::Machine;
use tandem_proto::CursorSample;
use tandem_proto::cursor::MAX_HZ;
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::geometry::{Point, Side};

/// Separación mínima entre dos muestras. Redondeada hacia arriba (no hacia abajo) para que
/// en cualquier segundo quepan 30 y no 31.
pub const MIN_INTERVAL: Duration = Duration::from_micros(1_000_000_u64.div_ceil(MAX_HZ as u64));

/// Qué se publica: una muestra o un cruce.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CursorEvent {
    Sample(CursorSample),
    Crossed {
        from: Machine,
        to: Machine,
        /// El borde de `from` por el que salió.
        edge: Side,
    },
}

/// Un cruce de borde, tal como lo anota la sesión del servidor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Crossing {
    pub from: Machine,
    pub to: Machine,
    pub edge: Side,
}

/// Interruptor del cursor en vivo, compartido entre el agente y los drivers.
#[derive(Debug, Default)]
pub struct CursorGate(AtomicBool);

impl CursorGate {
    pub fn set(&self, on: bool) {
        self.0.store(on, Ordering::Relaxed);
    }

    /// Si hay quien mire. Es lo único que paga el camino caliente sin suscripciones.
    #[inline]
    pub fn is_on(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Por dónde salen las muestras y los cruces, y el interruptor que dice si hay quien mire.
#[derive(Debug, Clone)]
pub struct CursorTap {
    gate: Arc<CursorGate>,
    tx: mpsc::UnboundedSender<CursorEvent>,
}

impl CursorTap {
    /// El tap y el receptor de lo que publican los drivers. El canal no tiene tope porque
    /// el límite de [`MAX_HZ`] ya lo acota; quien lo lee es una tarea del agente.
    pub fn new(gate: Arc<CursorGate>) -> (Self, mpsc::UnboundedReceiver<CursorEvent>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (CursorTap { gate, tx }, rx)
    }
}

/// Dónde está el cursor, sin la pantalla: lo barato de comparar y de guardar mientras se
/// espera el turno de publicarlo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Spot {
    pub machine: Machine,
    pub x: i32,
    pub y: i32,
}

impl Spot {
    pub fn new(machine: Machine, p: Point) -> Self {
        // `as` satura al rango de i32, que para un punto de pantalla sobra.
        Spot {
            machine,
            x: p.x.round() as i32,
            y: p.y.round() as i32,
        }
    }
}

/// Limita la frecuencia de lo que se publica. Trabaja con el tiempo que le pasan (en
/// microsegundos desde un origen cualquiera), para probarlo sin reloj.
///
/// Garantiza que dos salidas distan al menos `min_us`, que lo repetido no sale y que lo
/// frenado sale después con el valor más nuevo.
#[derive(Debug, Clone)]
pub struct Limiter<T> {
    min_us: u64,
    last_at: Option<u64>,
    sent: Option<T>,
    pending: Option<T>,
}

impl<T: Copy + PartialEq> Limiter<T> {
    pub fn new(min_us: u64) -> Self {
        Limiter {
            min_us,
            last_at: None,
            sent: None,
            pending: None,
        }
    }

    fn ready(&self, now_us: u64) -> bool {
        self.last_at
            .is_none_or(|last| now_us.saturating_sub(last) >= self.min_us)
    }

    fn emit(&mut self, now_us: u64, value: T) -> Option<T> {
        self.last_at = Some(now_us);
        self.sent = Some(value);
        self.pending = None;
        Some(value)
    }

    /// Un valor nuevo: sale ya si toca y no es el que salió; si no, queda pendiente.
    pub fn offer(&mut self, now_us: u64, value: T) -> Option<T> {
        if self.sent == Some(value) {
            self.pending = None;
            return None;
        }
        if self.ready(now_us) {
            return self.emit(now_us, value);
        }
        self.pending = Some(value);
        None
    }

    /// Cuándo puede salir lo pendiente; `None` si no hay nada pendiente.
    pub fn due_at(&self) -> Option<u64> {
        let last = self.last_at?;
        self.pending.map(|_| last.saturating_add(self.min_us))
    }

    /// Saca lo pendiente si ya toca.
    pub fn flush(&mut self, now_us: u64) -> Option<T> {
        let value = self.pending?;
        if !self.ready(now_us) {
            return None;
        }
        self.emit(now_us, value)
    }

    /// Olvida solo lo pendiente.
    pub fn drop_pending(&mut self) {
        self.pending = None;
    }

    /// Olvida lo pendiente y lo último enviado: lo de antes ya no le sirve a quien vuelva a
    /// mirar (y a quien mira ahora tampoco, si el interruptor se apagó).
    pub fn reset(&mut self) {
        self.sent = None;
        self.pending = None;
    }
}

/// Lo que un driver lleva para publicar el cursor.
#[derive(Debug)]
pub struct CursorFeed {
    tap: CursorTap,
    /// La máquina de este driver: las muestras que publica son de su cursor.
    machine: Machine,
    limiter: Limiter<Spot>,
    epoch: Instant,
    on: bool,
}

impl CursorFeed {
    pub fn new(tap: CursorTap, machine: Machine) -> Self {
        CursorFeed {
            tap,
            machine,
            limiter: Limiter::new(MIN_INTERVAL.as_micros() as u64),
            epoch: Instant::now(),
            on: false,
        }
    }

    pub fn machine(&self) -> Machine {
        self.machine
    }

    fn now_us(&self) -> u64 {
        Instant::now()
            .saturating_duration_since(self.epoch)
            .as_micros() as u64
    }

    /// ¿Hay quien mire? Es la comprobación del camino caliente. Al apagarse se descarta lo
    /// pendiente.
    #[inline]
    pub fn active(&mut self) -> bool {
        let on = self.tap.gate.is_on();
        if on != self.on {
            self.on = on;
            if !on {
                self.limiter.reset();
            }
        }
        on
    }

    /// El cursor está en `p` (de la máquina de este driver). `screen` dice en qué pantalla
    /// cae, y solo se llama si la muestra sale.
    pub fn observe(&mut self, p: Point, screen: impl FnOnce(Point) -> String) {
        let spot = Spot::new(self.machine, p);
        let now = self.now_us();
        if let Some(spot) = self.limiter.offer(now, spot) {
            self.send_sample(spot, screen);
        }
    }

    /// Descarta la muestra que se frenó: el foco se fue y ya no es de esta máquina.
    pub fn discard(&mut self) {
        self.limiter.drop_pending();
    }

    /// Cuándo puede salir la muestra que se frenó.
    pub fn next_due(&self) -> Option<Instant> {
        self.limiter
            .due_at()
            .map(|us| self.epoch + Duration::from_micros(us))
    }

    /// Saca la muestra frenada si ya toca.
    pub fn flush(&mut self, screen: impl FnOnce(Point) -> String) {
        let now = self.now_us();
        if let Some(spot) = self.limiter.flush(now) {
            self.send_sample(spot, screen);
        }
    }

    fn send_sample(&self, spot: Spot, screen: impl FnOnce(Point) -> String) {
        let screen = screen(Point::new(f64::from(spot.x), f64::from(spot.y)));
        // Sin receptor el agente ya se fue: no hay a quién avisar.
        let _ = self.tap.tx.send(CursorEvent::Sample(CursorSample {
            machine: spot.machine,
            screen,
            x: spot.x,
            y: spot.y,
        }));
    }

    /// Un cruce: no se limita, es raro y la UI no lo puede perder.
    pub fn crossed(&self, c: Crossing) {
        let _ = self.tap.tx.send(CursorEvent::Crossed {
            from: c.from,
            to: c.to,
            edge: c.edge,
        });
    }
}

/// Espera hasta `due`; sin fecha, nunca. Para el `select!` de los drivers.
pub(super) async fn sleep_until_opt(due: Option<Instant>) {
    match due {
        Some(t) => tokio::time::sleep_until(t).await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    const MIN: u64 = 33_334;

    fn limiter() -> Limiter<u32> {
        Limiter::new(MIN)
    }

    #[test]
    fn the_minimum_interval_fits_thirty_samples_a_second() {
        assert_eq!(MIN_INTERVAL, Duration::from_micros(MIN));
        // Con 33 333 µs entrarían 31 en un segundo.
        assert!(30 * MIN_INTERVAL.as_micros() >= 1_000_000);
    }

    #[test]
    fn the_first_value_goes_out_at_once_and_a_close_one_waits() {
        let mut l = limiter();
        assert_eq!(l.offer(0, 1), Some(1));
        assert_eq!(l.offer(10, 2), None);
        assert_eq!(l.due_at(), Some(MIN));
        assert_eq!(l.flush(MIN - 1), None);
        assert_eq!(l.flush(MIN), Some(2));
        assert_eq!(l.due_at(), None);
        assert_eq!(l.flush(10 * MIN), None);
    }

    #[test]
    fn a_waiting_value_is_replaced_by_the_newest() {
        let mut l = limiter();
        l.offer(0, 1);
        assert_eq!(l.offer(5, 2), None);
        assert_eq!(l.offer(9, 3), None);
        assert_eq!(l.flush(MIN), Some(3));
    }

    #[test]
    fn what_was_already_sent_is_not_repeated() {
        let mut l = limiter();
        assert_eq!(l.offer(0, 1), Some(1));
        assert_eq!(l.offer(MIN, 1), None);
        assert_eq!(l.offer(2 * MIN, 1), None);
        // Volver a lo anterior mientras otro espera cancela la espera.
        assert_eq!(l.offer(2 * MIN + 1, 2), Some(2));
        assert_eq!(l.offer(2 * MIN + 2, 3), None);
        assert_eq!(l.offer(2 * MIN + 3, 2), None);
        assert_eq!(l.flush(10 * MIN), None);
    }

    #[test]
    fn resetting_forgets_the_pending_value_and_the_last_one_sent() {
        let mut l = limiter();
        l.offer(0, 1);
        l.offer(1, 2);
        l.reset();
        assert_eq!(l.due_at(), None);
        // Quien vuelva a mirar quiere ver dónde está el cursor aunque no se haya movido,
        // pero sin saltarse el intervalo.
        assert_eq!(l.offer(2, 1), None);
        assert_eq!(l.flush(MIN), Some(1));
    }

    #[derive(Debug, Clone)]
    enum Step {
        Offer(u32),
        Flush,
    }

    fn steps() -> impl Strategy<Value = Vec<(u64, Step)>> {
        // Saltos de tiempo desde 0 hasta 100 ms, con muchos más chicos que grandes: el
        // mouse a 1000 Hz y pausas.
        let gap = prop_oneof![8 => 0u64..2_000, 2 => 0u64..100_000];
        let step = prop_oneof![
            3 => (0u32..50).prop_map(Step::Offer),
            1 => Just(Step::Flush),
        ];
        prop::collection::vec((gap, step), 0..400).prop_map(|v| {
            let mut t = 0;
            v.into_iter()
                .map(|(g, s)| {
                    t += g;
                    (t, s)
                })
                .collect()
        })
    }

    proptest! {
        /// El invariante de T-128: nunca más de 30 salidas en cualquier segundo, pase lo
        /// que pase con las ofertas y los avisos del reloj.
        #[test]
        fn never_more_than_thirty_a_second(steps in steps()) {
            let mut l = limiter();
            let mut out = Vec::new();
            for (t, step) in steps {
                let got = match step {
                    Step::Offer(v) => l.offer(t, v),
                    Step::Flush => l.flush(t),
                };
                if got.is_some() {
                    out.push(t);
                }
            }
            for w in out.windows(31) {
                prop_assert!(w[30] - w[0] >= 1_000_000, "31 salidas en {} µs", w[30] - w[0]);
            }
            for w in out.windows(2) {
                prop_assert!(w[1] - w[0] >= MIN);
            }
        }

        /// Lo último que se ofreció siempre sale (a lo más un intervalo después): el punto
        /// de la UI termina donde el mouse se detuvo.
        #[test]
        fn the_last_value_always_gets_out(steps in steps(), last in 100u32..200) {
            let mut l = limiter();
            let mut now = 0;
            for (t, step) in steps {
                now = t;
                match step {
                    Step::Offer(v) => {
                        l.offer(t, v);
                    }
                    Step::Flush => {
                        l.flush(t);
                    }
                }
            }
            let got = l.offer(now, last);
            let got = got.or_else(|| l.due_at().and_then(|due| l.flush(due)));
            prop_assert_eq!(got, Some(last));
        }
    }

    #[test]
    fn a_spot_rounds_to_whole_pixels() {
        let s = Spot::new(Machine::A, Point::new(10.4, -0.6));
        assert_eq!((s.x, s.y), (10, -1));
        let s = Spot::new(Machine::B, Point::new(f64::MAX, f64::MIN));
        assert_eq!((s.x, s.y), (i32::MAX, i32::MIN));
    }

    #[tokio::test(start_paused = true)]
    async fn a_feed_publishes_only_while_the_gate_is_on_and_at_most_thirty_hertz() {
        let gate = Arc::new(CursorGate::default());
        let (tap, mut rx) = CursorTap::new(gate.clone());
        let mut feed = CursorFeed::new(tap, Machine::B);
        let screen = |_: Point| "b-1".to_owned();

        assert!(!feed.active());
        // El driver no llama a `observe` con el interruptor apagado; si lo hiciera por
        // error, aun así `active` devuelve falso y nada se publica.
        assert!(rx.try_recv().is_err());

        gate.set(true);
        assert!(feed.active());
        feed.observe(Point::new(1.0, 2.0), screen);
        feed.observe(Point::new(3.0, 4.0), screen);
        assert_eq!(
            rx.try_recv(),
            Ok(CursorEvent::Sample(CursorSample {
                machine: Machine::B,
                screen: "b-1".into(),
                x: 1,
                y: 2,
            }))
        );
        assert!(rx.try_recv().is_err(), "la segunda espera su turno");
        let due = feed.next_due().expect("hay una muestra frenada");
        tokio::time::sleep_until(due).await;
        feed.flush(screen);
        assert!(matches!(
            rx.try_recv(),
            Ok(CursorEvent::Sample(CursorSample { x: 3, y: 4, .. }))
        ));

        // Al apagarse, lo pendiente se descarta.
        feed.observe(Point::new(5.0, 6.0), screen);
        gate.set(false);
        assert!(!feed.active());
        assert_eq!(feed.next_due(), None);
    }

    #[tokio::test(start_paused = true)]
    async fn a_crossing_is_never_held_back() {
        let gate = Arc::new(CursorGate::default());
        gate.set(true);
        let (tap, mut rx) = CursorTap::new(gate);
        let feed = CursorFeed::new(tap, Machine::A);
        let c = Crossing {
            from: Machine::A,
            to: Machine::B,
            edge: Side::Right,
        };
        feed.crossed(c);
        feed.crossed(c);
        for _ in 0..2 {
            assert_eq!(
                rx.try_recv(),
                Ok(CursorEvent::Crossed {
                    from: Machine::A,
                    to: Machine::B,
                    edge: Side::Right,
                })
            );
        }
    }
}
