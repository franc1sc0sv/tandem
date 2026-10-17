//! Backend DDC/CI: escribe `0x60` con el código del perfil del monitor.

use std::sync::Arc;
use std::time::Duration;

use tandem_core::MonitorConfig;

use super::{MonitorSwitcher, SwitchError, SwitchFuture, SwitchRequest};
use crate::ddc::switch::{RETRY_GAP, RETURN_ATTEMPTS};
use crate::ddc::{DdcBus, DdcError, VCP_INPUT_SOURCE};

/// Abre el bus DDC. Se llama en cada cambio: la lista de monitores puede haber cambiado
/// (y en Windows los handles no sobreviven a todo cambio de topología, spike S3).
pub type BusFactory = Arc<dyn Fn() -> Result<Box<dyn DdcBus>, DdcError> + Send + Sync>;

/// Cambia entradas por DDC/CI, buscando el monitor por el `edid_model` de la configuración.
pub struct DdcSwitcher {
    open: BusFactory,
    attempts: u32,
    retry_gap: Duration,
    /// Turno de las escrituras. Si un cambio vence, su hilo bloqueante sigue
    /// escribiendo aunque nadie lo espere; el siguiente (su inverso, típicamente)
    /// tiene que esperar a que termine, o la escritura vieja llega última y deja
    /// el monitor en la entrada equivocada.
    turn: Arc<tokio::sync::Mutex<()>>,
}

impl DdcSwitcher {
    /// Con los reintentos de `tandem-ddc`: los errores sueltos de DDC son frecuentes.
    pub fn new(open: BusFactory) -> Self {
        Self::with_retries(open, RETURN_ATTEMPTS, RETRY_GAP)
    }

    pub fn with_retries(open: BusFactory, attempts: u32, retry_gap: Duration) -> Self {
        DdcSwitcher {
            open,
            attempts: attempts.max(1),
            retry_gap,
            turn: Arc::default(),
        }
    }

    /// El backend real de la PC.
    #[cfg(windows)]
    pub fn windows() -> Self {
        Self::new(Arc::new(|| {
            crate::windows::ddc::WindowsDdc::open().map(|b| Box::new(b) as Box<dyn DdcBus>)
        }))
    }
}

impl MonitorSwitcher for DdcSwitcher {
    fn switch<'a>(&'a self, req: &'a SwitchRequest) -> SwitchFuture<'a> {
        let open = self.open.clone();
        let (attempts, gap) = (self.attempts, self.retry_gap);
        let turn = self.turn.clone();
        let monitor = req.monitor.clone();
        let input = req.input;
        Box::pin(async move {
            let id = monitor.id.clone();
            let code = monitor
                .ddc
                .as_ref()
                .ok_or_else(|| SwitchError::NoProfile(id.clone()))?
                .code_for(input);
            let turn = turn.lock_owned().await;
            // DDC/CI bloquea (cada escritura tarda decenas de ms y los reintentos esperan).
            tokio::task::spawn_blocking(move || {
                let _turn = turn;
                let mut bus = open().map_err(|error| SwitchError::Ddc {
                    monitor: monitor.id.clone(),
                    error,
                })?;
                write_input(bus.as_mut(), &monitor, code, attempts, gap)
            })
            .await
            .map_err(|e| SwitchError::Internal {
                monitor: id,
                message: format!("el cambio por DDC terminó mal: {e}"),
            })?
        })
    }
}

fn find(bus: &dyn DdcBus, monitor: &MonitorConfig) -> Result<usize, SwitchError> {
    let model = monitor
        .edid_model
        .as_deref()
        .ok_or_else(|| SwitchError::NoEdidModel(monitor.id.clone()))?;
    let hits: Vec<usize> = bus
        .monitors()
        .iter()
        .filter(|m| monitor.models().any(|alias| m.matches(alias)))
        .map(|m| m.index)
        .collect();
    match hits[..] {
        [index] => Ok(index),
        [] => Err(SwitchError::NotFound {
            monitor: monitor.id.clone(),
            model: model.to_owned(),
        }),
        _ => Err(SwitchError::Ambiguous {
            monitor: monitor.id.clone(),
            model: model.to_owned(),
            count: hits.len(),
        }),
    }
}

fn write_input(
    bus: &mut dyn DdcBus,
    monitor: &MonitorConfig,
    code: u8,
    attempts: u32,
    gap: Duration,
) -> Result<(), SwitchError> {
    let index = find(bus, monitor)?;
    let mut last = None;
    for attempt in 1..=attempts {
        match bus.set_vcp(index, VCP_INPUT_SOURCE, u32::from(code)) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last = Some(e);
                if attempt < attempts && !gap.is_zero() {
                    std::thread::sleep(gap);
                }
            }
        }
    }
    Err(SwitchError::Ddc {
        monitor: monitor.id.clone(),
        error: last.unwrap_or(DdcError::Unsupported),
    })
}

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, mpsc};

    use tandem_core::Machine;

    use super::*;
    use crate::ddc::fake::{FakeDdc, FakeMonitor, SharedFakeDdc, Write};
    use crate::ddc::{Edid, MonitorInfo};
    use crate::switcher::tests::request;

    fn named(name: &str) -> FakeMonitor {
        FakeMonitor {
            info: Some(MonitorInfo {
                index: 0,
                description: "Monitor PnP genérico".into(),
                device: String::new(),
                edid: Some(Edid {
                    manufacturer: "SAM".into(),
                    product_code: 0x7259,
                    serial_number: 0,
                    name: Some(name.into()),
                    serial_text: None,
                    size_cm: None,
                }),
            }),
            ..Default::default()
        }
    }

    /// Los monitores dados y un guion de escrituras.
    fn bus(monitors: Vec<FakeMonitor>, script: &[bool]) -> (DdcSwitcher, SharedFakeDdc) {
        let mut fake = FakeDdc::new(monitors);
        fake.write_script = script.iter().copied().collect();
        let shared = SharedFakeDdc::new(fake);
        (
            DdcSwitcher::with_retries(shared.factory(), 3, Duration::ZERO),
            shared,
        )
    }

    fn writes(bus: &SharedFakeDdc) -> Vec<Write> {
        bus.writes()
    }

    fn desk_bus(script: &[bool]) -> (DdcSwitcher, SharedFakeDdc) {
        bus(vec![named("S32BG85"), named("LC34G55T")], script)
    }

    #[tokio::test]
    async fn writes_the_profile_code_to_the_matching_monitor() {
        let (s, inner) = desk_bus(&[]);
        s.switch(&request("m34", Machine::Mac)).await.unwrap();
        s.switch(&request("m34", Machine::Pc)).await.unwrap();
        assert_eq!(
            writes(&inner),
            [
                (1, VCP_INPUT_SOURCE, 0x06, true),
                (1, VCP_INPUT_SOURCE, 0x0F, true)
            ]
        );
    }

    #[tokio::test]
    async fn retries_flaky_writes() {
        let (s, inner) = desk_bus(&[false, false]);
        s.switch(&request("m34", Machine::Mac)).await.unwrap();
        assert_eq!(writes(&inner).len(), 3);
    }

    #[tokio::test]
    async fn gives_up_after_the_last_attempt() {
        let (s, inner) = desk_bus(&[false, false, false]);
        let err = s.switch(&request("m34", Machine::Mac)).await.unwrap_err();
        assert!(matches!(err, SwitchError::Ddc { .. }), "{err:?}");
        assert!(
            err.to_string().starts_with("m34: SetVCPFeature falló"),
            "{err}"
        );
        assert_eq!(writes(&inner).len(), 3);
    }

    #[tokio::test]
    async fn does_not_write_without_a_unique_match() {
        let (s, inner) = bus(vec![named("S32BG85")], &[]);
        let err = s.switch(&request("m34", Machine::Mac)).await.unwrap_err();
        assert!(matches!(err, SwitchError::NotFound { .. }), "{err:?}");

        let (s2, inner2) = bus(vec![named("LC34G55T"), named("LC34G55T")], &[]);
        let err = s2.switch(&request("m34", Machine::Mac)).await.unwrap_err();
        assert!(
            matches!(err, SwitchError::Ambiguous { count: 2, .. }),
            "{err:?}"
        );

        let mut no_model = request("m34", Machine::Mac);
        no_model.monitor.edid_model = None;
        let err = s.switch(&no_model).await.unwrap_err();
        assert_eq!(err, SwitchError::NoEdidModel("m34".into()));

        let err = s.switch(&request("m32", Machine::Mac)).await.unwrap_err();
        assert_eq!(err, SwitchError::NoProfile("m32".into()));

        assert!(writes(&inner).is_empty());
        assert!(writes(&inner2).is_empty());
    }

    #[tokio::test]
    async fn reports_a_bus_that_cannot_be_opened() {
        let s = DdcSwitcher::new(Arc::new(|| Err(DdcError::Unsupported)));
        let err = s.switch(&request("m34", Machine::Mac)).await.unwrap_err();
        assert_eq!(
            err.to_string(),
            "m34: DDC/CI no está disponible en esta plataforma"
        );
    }

    /// Aviso de que empezó, permiso para seguir y aviso de que terminó.
    type Gate = (mpsc::Sender<()>, mpsc::Receiver<()>, mpsc::Sender<()>);

    /// Un bus cuya primera escritura se queda bloqueada hasta que la prueba la suelta.
    struct GatedBus {
        inner: SharedFakeDdc,
        gate: Arc<Mutex<Option<Gate>>>,
    }

    impl DdcBus for GatedBus {
        fn monitors(&self) -> &[MonitorInfo] {
            self.inner.monitors()
        }
        fn capabilities(&mut self, monitor: usize) -> Result<String, DdcError> {
            self.inner.capabilities(monitor)
        }
        fn get_vcp(&mut self, monitor: usize, code: u8) -> Result<crate::ddc::VcpValue, DdcError> {
            self.inner.get_vcp(monitor, code)
        }
        fn set_vcp(&mut self, monitor: usize, code: u8, value: u32) -> Result<(), DdcError> {
            let gate = self.gate.lock().unwrap().take();
            if let Some((started, release, _)) = &gate {
                started.send(()).unwrap();
                release.recv().unwrap();
            }
            let r = self.inner.set_vcp(monitor, code, value);
            if let Some((_, _, done)) = gate {
                done.send(()).unwrap();
            }
            r
        }
    }

    // QA ronda 2: un cambio que vence sigue escribiendo en su hilo bloqueante. El
    // inverso, pedido justo después, escribía antes y la escritura vieja llegaba
    // última: el monitor quedaba en la entrada de la que se quería volver.
    #[tokio::test]
    async fn a_timed_out_switch_cannot_land_after_the_next_one() {
        let (started_tx, started) = mpsc::channel();
        let (release, release_rx) = mpsc::channel();
        let (done_tx, done) = mpsc::channel();
        let inner = SharedFakeDdc::new(FakeDdc::new(vec![named("S32BG85"), named("LC34G55T")]));
        let gate = Arc::new(Mutex::new(Some((started_tx, release_rx, done_tx))));
        let bus = {
            let (inner, gate) = (inner.clone(), gate.clone());
            Arc::new(move || {
                Ok(Box::new(GatedBus {
                    inner: inner.clone(),
                    gate: gate.clone(),
                }) as Box<dyn DdcBus>)
            })
        };
        let s = Arc::new(DdcSwitcher::with_retries(bus, 1, Duration::ZERO));

        // A la PC (DP, 0x0F): la escritura se cuelga y el paso vence (se suelta el futuro).
        let to_pc = request("m34", Machine::Pc);
        {
            let mut first = s.switch(&to_pc);
            tokio::select! {
                biased;
                _ = &mut first => panic!("la escritura colgada terminó"),
                _ = tokio::task::spawn_blocking(move || started.recv()) => {}
            }
        }
        // El inverso, a la Mac (HDMI, 0x06).
        let back = tokio::spawn({
            let s = s.clone();
            async move { s.switch(&request("m34", Machine::Mac)).await }
        });
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        release.send(()).unwrap();
        back.await.unwrap().unwrap();
        tokio::task::spawn_blocking(move || done.recv())
            .await
            .unwrap()
            .unwrap();
        let codes: Vec<u32> = writes(&inner).iter().map(|w| w.2).collect();
        assert_eq!(codes, [0x0F, 0x06], "la última escritura manda");
    }
}
