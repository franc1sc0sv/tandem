//! Calibración DDC: encontrar los códigos de `0x60` de un monitor desconocido.
//!
//! Los Samsung del puesto declaran unos códigos y obedecen otros, y la lectura de `0x60`
//! no dice cuándo cambió la entrada (`docs/research/ddc-34-LC34G55T.md`,
//! `ddc-32-odyssey-g5.md`). La única prueba fiable es que alguien vea la máquina destino
//! en el monitor: eso lo responde un [`Confirmer`] (T-078 lo hace desde la otra máquina).
//!
//! Recorrido, con la regla de que nunca se prueba un código de ida sin tener verificado
//! el de vuelta:
//!
//! 1. Se lee el capabilities string y `0x60` en la entrada de partida.
//! 2. Se le pide al usuario que pase el monitor a la otra entrada desde su menú
//!    ([`ProbeKind::UserSwitch`]) y se lee `0x60` ahí (la pista que resolvió el 32").
//! 3. Desde la otra entrada se prueban los candidatos de la de partida hasta que la
//!    máquina de partida se vea: ese es el código de vuelta verificado.
//! 4. Se prueban los candidatos de la otra entrada, y después de cada uno se vuelve con
//!    el código verificado, se haya visto o no.
//!
//! Si algo sale mal antes de tener el código de vuelta, o la vuelta falla, se le pide al
//! usuario que regrese el monitor a mano. Cancelar ([`CancelToken`]) corta la espera en
//! curso y deja el monitor en la entrada de partida por el mismo camino.
//!
//! El bus se usa directamente (cada operación DDC tarda decenas de ms): quien lo corra
//! en un runtime compartido debería darle un hilo propio.

pub mod candidates;
pub mod console;
pub mod profile;
pub mod sim;

#[cfg(test)]
mod tests;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tandem_core::{DdcProfile, Input};
use tokio::sync::Notify;

use crate::ddc::switch::{InputCodes, RETRY_GAP, RETURN_ATTEMPTS, other_input};
use crate::ddc::{DdcBus, DdcError, MonitorInfo, VCP_INPUT_SOURCE, caps};

pub use candidates::{BrandProfile, Candidate, Source};

/// Tiempo para que el usuario diga que ve la máquina destino tras un cambio por DDC.
pub const CONFIRM_TIMEOUT: Duration = Duration::from_secs(8);
/// Tiempo para que el usuario cambie la entrada a mano desde el menú del monitor.
pub const USER_SWITCH_TIMEOUT: Duration = Duration::from_secs(120);
/// Pausa tras la vuelta para que el monitor resincronice antes del siguiente código.
pub const SETTLE: Duration = Duration::from_secs(3);
/// Lecturas de `0x60` por entrada: los errores sueltos de DDC son frecuentes.
pub const READ_ATTEMPTS: u32 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalibrationOptions {
    pub confirm_timeout: Duration,
    pub user_switch_timeout: Duration,
    pub settle: Duration,
    pub write_attempts: u32,
    pub retry_gap: Duration,
}

impl Default for CalibrationOptions {
    fn default() -> Self {
        Self {
            confirm_timeout: CONFIRM_TIMEOUT,
            user_switch_timeout: USER_SWITCH_TIMEOUT,
            settle: SETTLE,
            write_attempts: RETURN_ATTEMPTS,
            retry_gap: RETRY_GAP,
        }
    }
}

/// Qué monitor calibrar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalibrationRequest {
    /// Índice en [`DdcBus::monitors`].
    pub monitor: usize,
    /// La entrada en la que está el monitor al empezar (en la PC, su DisplayPort).
    pub start: Input,
    /// Perfiles verificados de la base; solo se usan los del mismo fabricante.
    pub known: Vec<BrandProfile>,
    pub options: CalibrationOptions,
}

/// Qué tiene que confirmar el usuario.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeKind {
    /// El motor acaba de mandar `code`: ¿se ve la máquina de esa entrada?
    Candidate { code: u8 },
    /// El usuario tiene que cambiar la entrada desde el menú del monitor y confirmar
    /// cuando vea la máquina de esa entrada.
    UserSwitch,
}

/// Una pregunta al [`Confirmer`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    pub monitor: MonitorInfo,
    /// La entrada que debería verse. La máquina que la muestra sale del cableado.
    pub input: Input,
    pub kind: ProbeKind,
    /// Lo que espera el motor: pasado este tiempo cuenta como no visto.
    pub timeout: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ConfirmError(pub String);

pub type ConfirmFuture<'a> = Pin<Box<dyn Future<Output = Result<bool, ConfirmError>> + Send + 'a>>;

/// ¿Se ve la máquina destino en el monitor?
///
/// `Ok(true)` = el usuario la vio. Puede no terminar nunca si nadie contesta: el motor
/// corta a los [`Probe::timeout`] y lo cuenta como no visto (y suelta el futuro, así que
/// al soltarlo hay que quitar la sonda).
pub trait Confirmer: Send + Sync {
    fn confirm<'a>(&'a self, probe: &'a Probe) -> ConfirmFuture<'a>;
}

/// Cancelación cooperativa de una calibración. Clonable: la UI cancela, el motor espera.
#[derive(Debug, Clone, Default)]
pub struct CancelToken {
    inner: Arc<(AtomicBool, Notify)>,
}

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.inner.0.store(true, Ordering::SeqCst);
        self.inner.1.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.0.load(Ordering::SeqCst)
    }

    pub async fn cancelled(&self) {
        loop {
            let notified = self.inner.1.notified();
            tokio::pin!(notified);
            // Registrarse antes de mirar la bandera: si no, un `cancel` entre las dos
            // cosas se perdería.
            notified.as_mut().enable();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }
}

/// Etapa de la calibración.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Leyendo capabilities y `0x60`.
    Reading,
    /// Esperando a que el usuario cambie la entrada a mano.
    UserSwitch {
        input: Input,
    },
    /// Probando candidatos de una entrada.
    Testing {
        input: Input,
    },
    Done,
    Cancelled,
    Failed,
}

/// Progreso observable de la calibración.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CalibrationEvent {
    State(State),
    /// Valores de `0x60` del capabilities string (`None` si no se pudo leer o interpretar).
    Declared(Option<Vec<u8>>),
    /// Lectura de `0x60` estando en `input`.
    Reported {
        input: Input,
        value: Option<u8>,
    },
    /// Se manda un candidato: `attempt` de `of` para esa entrada.
    Trying {
        input: Input,
        candidate: Candidate,
        attempt: usize,
        of: usize,
    },
    /// Respuesta del `Confirmer` a un candidato.
    Seen {
        input: Input,
        code: u8,
        seen: bool,
    },
    /// Vuelta a la entrada de partida con el código verificado.
    Returned {
        code: u8,
    },
    Found {
        input: Input,
        code: u8,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CalibrationError {
    #[error(transparent)]
    Ddc(#[from] DdcError),
    #[error("ningún código funcionó para {}", crate::ddc::switch::input_name(*.input))]
    NoWorkingCode { input: Input },
    #[error(
        "no se confirmó el cambio manual a {} desde el menú del monitor",
        crate::ddc::switch::input_name(*.input)
    )]
    UserDidNotSwitch { input: Input },
    #[error("calibración cancelada")]
    Cancelled,
    #[error("la confirmación falló: {0}")]
    Confirmer(ConfirmError),
    #[error("no se pudo mandar la vuelta con {code:#04X}: {error}")]
    ReturnFailed { code: u8, error: DdcError },
    /// El monitor pudo quedar fuera de la entrada de partida: hay que regresarlo a mano.
    #[error(
        "el monitor puede haber quedado fuera de {}: regrésalo desde su menú",
        crate::ddc::switch::input_name(*.start)
    )]
    NotReturned { start: Input },
}

/// Lectura de `0x60` en cada entrada, para la base de perfiles (T-079).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reported {
    pub hdmi: Option<u8>,
    pub display_port: Option<u8>,
}

impl Reported {
    pub fn for_input(&self, input: Input) -> Option<u8> {
        match input {
            Input::Hdmi => self.hdmi,
            Input::DisplayPort => self.display_port,
        }
    }

    fn set(&mut self, input: Input, value: Option<u8>) {
        match input {
            Input::Hdmi => self.hdmi = value,
            Input::DisplayPort => self.display_port = value,
        }
    }
}

/// Resultado: un código verificado por entrada y lo que se supo del monitor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalibratedProfile {
    /// Un solo código por entrada, el que se vio funcionar.
    pub codes: InputCodes,
    pub pnp_id: Option<String>,
    pub edid_name: Option<String>,
    pub description: String,
    pub start: Input,
    pub reported: Reported,
    /// Valores de `0x60` declarados en el capabilities string.
    pub declared: Vec<u8>,
    pub mccs_ver: Option<String>,
    /// Candidatos probados en total (de las dos entradas).
    pub attempts: usize,
}

impl CalibratedProfile {
    /// El perfil para `[[monitors]].ddc` de la configuración.
    pub fn ddc_profile(&self) -> Option<DdcProfile> {
        Some(DdcProfile {
            hdmi: *self.codes.hdmi.first()?,
            display_port: *self.codes.display_port.first()?,
        })
    }
}

/// Calibra un monitor. Siempre termina con el monitor en `req.start`, salvo
/// [`CalibrationError::NotReturned`] (y [`CalibrationError::Ddc`] antes de tocar nada).
pub async fn calibrate(
    bus: &mut (dyn DdcBus + Send),
    req: &CalibrationRequest,
    confirmer: &dyn Confirmer,
    cancel: &CancelToken,
    on_event: &mut (dyn FnMut(&CalibrationEvent) + Send),
) -> Result<CalibratedProfile, CalibrationError> {
    let info = bus
        .monitors()
        .get(req.monitor)
        .cloned()
        .ok_or(DdcError::NoSuchMonitor(req.monitor))?;
    let mut run = Run {
        bus,
        req,
        info,
        confirmer,
        cancel,
        on_event,
        home: None,
        away: false,
        attempts: 0,
    };
    let result = run.calibrate().await;
    let end = match &result {
        Ok(_) => State::Done,
        Err(CalibrationError::Cancelled) => State::Cancelled,
        Err(_) => State::Failed,
    };
    (run.on_event)(&CalibrationEvent::State(end));
    result
}

struct Run<'a> {
    bus: &'a mut (dyn DdcBus + Send),
    req: &'a CalibrationRequest,
    info: MonitorInfo,
    confirmer: &'a dyn Confirmer,
    cancel: &'a CancelToken,
    on_event: &'a mut (dyn FnMut(&CalibrationEvent) + Send),
    /// Código verificado de la entrada de partida.
    home: Option<u8>,
    /// El monitor puede no estar en la entrada de partida.
    away: bool,
    attempts: usize,
}

/// Cómo terminó una espera cancelable.
enum Waited<T> {
    Done(T),
    Cancelled,
}

impl Run<'_> {
    fn emit(&mut self, event: CalibrationEvent) {
        (self.on_event)(&event);
    }

    async fn calibrate(&mut self) -> Result<CalibratedProfile, CalibrationError> {
        match self.steps().await {
            Ok(profile) => Ok(profile),
            Err(error) => Err(self.go_home(error).await),
        }
    }

    async fn steps(&mut self) -> Result<CalibratedProfile, CalibrationError> {
        let start = self.req.start;
        let other = other_input(start);
        let mut reported = Reported::default();

        self.emit(CalibrationEvent::State(State::Reading));
        let (declared, mccs_ver) = self.read_capabilities();
        self.emit(CalibrationEvent::Declared(declared.clone()));
        let declared = declared.unwrap_or_default();
        self.read_reported(start, &mut reported);
        self.check_cancel()?;

        // Paso a la otra entrada a mano: es lo que permite verificar la vuelta antes de
        // mandar ningún código que la deje.
        self.emit(CalibrationEvent::State(State::UserSwitch { input: other }));
        self.away = true;
        let switched = self.ask(other, ProbeKind::UserSwitch).await?;
        if !switched {
            return Err(CalibrationError::UserDidNotSwitch { input: other });
        }
        self.read_reported(other, &mut reported);

        let manufacturer = self.info.edid.as_ref().map(|e| e.manufacturer.clone());
        let hints = |input: Input| candidates::Hints {
            reported: reported.for_input(input),
            declared: declared.clone(),
            same_brand: candidates::brand_codes(&self.req.known, manufacturer.as_deref(), input),
        };

        let (start_hints, other_hints) = (hints(start), hints(other));
        // La vuelta: desde la otra entrada, hasta que se vea la de partida.
        let home = self.find_home(&start_hints).await?;
        let target = self.find_target(other, &other_hints, home).await?;

        let mut codes = InputCodes {
            hdmi: Vec::new(),
            display_port: Vec::new(),
        };
        for (input, code) in [(start, home), (other, target)] {
            match input {
                Input::Hdmi => codes.hdmi.push(code),
                Input::DisplayPort => codes.display_port.push(code),
            }
        }
        let edid = self.info.edid.as_ref();
        Ok(CalibratedProfile {
            codes,
            pnp_id: edid.map(|e| e.pnp_id()),
            edid_name: edid.and_then(|e| e.name.clone()),
            description: self.info.description.clone(),
            start,
            reported,
            declared,
            mccs_ver,
            attempts: self.attempts,
        })
    }

    async fn find_home(&mut self, hints: &candidates::Hints) -> Result<u8, CalibrationError> {
        let start = self.req.start;
        self.emit(CalibrationEvent::State(State::Testing { input: start }));
        let list = candidates::ordered(start, hints, &[]);
        for (i, candidate) in list.iter().enumerate() {
            self.check_cancel()?;
            if !self.try_code(start, *candidate, i, list.len()).await {
                continue;
            }
            let seen = self
                .ask(
                    start,
                    ProbeKind::Candidate {
                        code: candidate.code,
                    },
                )
                .await?;
            self.emit(CalibrationEvent::Seen {
                input: start,
                code: candidate.code,
                seen,
            });
            if seen {
                self.home = Some(candidate.code);
                self.away = false;
                self.emit(CalibrationEvent::Found {
                    input: start,
                    code: candidate.code,
                });
                return Ok(candidate.code);
            }
        }
        Err(CalibrationError::NoWorkingCode { input: start })
    }

    async fn find_target(
        &mut self,
        input: Input,
        hints: &candidates::Hints,
        home: u8,
    ) -> Result<u8, CalibrationError> {
        self.emit(CalibrationEvent::State(State::Testing { input }));
        let list = candidates::ordered(input, hints, &[home]);
        for (i, candidate) in list.iter().enumerate() {
            self.check_cancel()?;
            // Aunque la escritura falle el monitor pudo obedecer: siempre se vuelve.
            self.away = true;
            let written = self.try_code(input, *candidate, i, list.len()).await;
            let seen = if written {
                let seen = self
                    .ask(
                        input,
                        ProbeKind::Candidate {
                            code: candidate.code,
                        },
                    )
                    .await?;
                self.emit(CalibrationEvent::Seen {
                    input,
                    code: candidate.code,
                    seen,
                });
                seen
            } else {
                false
            };
            self.return_home(home).await?;
            if seen {
                self.emit(CalibrationEvent::Found {
                    input,
                    code: candidate.code,
                });
                return Ok(candidate.code);
            }
        }
        Err(CalibrationError::NoWorkingCode { input })
    }

    /// Manda un candidato. `false` si ninguna escritura se aceptó.
    async fn try_code(&mut self, input: Input, candidate: Candidate, i: usize, of: usize) -> bool {
        self.attempts += 1;
        self.emit(CalibrationEvent::Trying {
            input,
            candidate,
            attempt: i + 1,
            of,
        });
        self.write(candidate.code).await.is_ok()
    }

    /// Escribe `0x60` con reintentos: los errores sueltos de DDC son frecuentes.
    async fn write(&mut self, code: u8) -> Result<(), DdcError> {
        let (attempts, gap) = (
            self.req.options.write_attempts.max(1),
            self.req.options.retry_gap,
        );
        let mut result = Err(DdcError::Unsupported);
        for attempt in 1..=attempts {
            if attempt > 1 {
                tokio::time::sleep(gap).await;
            }
            result = self
                .bus
                .set_vcp(self.req.monitor, VCP_INPUT_SOURCE, code.into());
            if result.is_ok() {
                break;
            }
        }
        result
    }

    /// Vuelta con el código verificado. No se puede cancelar: es lo que deja al usuario
    /// viendo la máquina de partida.
    async fn return_home(&mut self, code: u8) -> Result<(), CalibrationError> {
        self.write(code)
            .await
            .map_err(|error| CalibrationError::ReturnFailed { code, error })?;
        self.away = false;
        self.emit(CalibrationEvent::Returned { code });
        tokio::time::sleep(self.req.options.settle).await;
        Ok(())
    }

    /// Deja el monitor en la entrada de partida tras un error y decide qué se reporta.
    async fn go_home(&mut self, error: CalibrationError) -> CalibrationError {
        if !self.away {
            return error;
        }
        let start = self.req.start;
        if let Some(code) = self.home
            && !matches!(error, CalibrationError::ReturnFailed { .. })
            && self.return_home(code).await.is_ok()
        {
            return error;
        }
        // Sin código de vuelta (o sin bus): solo queda pedirlo a mano.
        self.emit(CalibrationEvent::State(State::UserSwitch { input: start }));
        let probe = self.probe(start, ProbeKind::UserSwitch);
        let back = matches!(
            tokio::time::timeout(probe.timeout, self.confirmer.confirm(&probe)).await,
            Ok(Ok(true))
        );
        if back {
            self.away = false;
            error
        } else {
            CalibrationError::NotReturned { start }
        }
    }

    fn probe(&self, input: Input, kind: ProbeKind) -> Probe {
        let opts = &self.req.options;
        Probe {
            monitor: self.info.clone(),
            input,
            kind,
            timeout: match kind {
                ProbeKind::Candidate { .. } => opts.confirm_timeout,
                ProbeKind::UserSwitch => opts.user_switch_timeout,
            },
        }
    }

    /// Pregunta al `Confirmer`, con su timeout y cancelable.
    async fn ask(&mut self, input: Input, kind: ProbeKind) -> Result<bool, CalibrationError> {
        let probe = self.probe(input, kind);
        let answer = tokio::time::timeout(probe.timeout, self.confirmer.confirm(&probe));
        match self.cancellable(answer).await {
            Waited::Cancelled => Err(CalibrationError::Cancelled),
            Waited::Done(Err(_elapsed)) => Ok(false),
            Waited::Done(Ok(Ok(seen))) => Ok(seen),
            Waited::Done(Ok(Err(e))) => Err(CalibrationError::Confirmer(e)),
        }
    }

    async fn cancellable<F: Future>(&self, f: F) -> Waited<F::Output> {
        tokio::select! {
            biased;
            () = self.cancel.cancelled() => Waited::Cancelled,
            out = f => Waited::Done(out),
        }
    }

    fn check_cancel(&self) -> Result<(), CalibrationError> {
        if self.cancel.is_cancelled() {
            Err(CalibrationError::Cancelled)
        } else {
            Ok(())
        }
    }

    fn read_capabilities(&mut self) -> (Option<Vec<u8>>, Option<String>) {
        match self
            .bus
            .capabilities(self.req.monitor)
            .ok()
            .and_then(|raw| caps::parse(&raw).ok())
        {
            Some(c) => (
                Some(
                    c.declared_values(VCP_INPUT_SOURCE)
                        .unwrap_or_default()
                        .to_vec(),
                ),
                c.mccs_ver,
            ),
            None => (None, None),
        }
    }

    fn read_reported(&mut self, input: Input, reported: &mut Reported) {
        let mut value = None;
        for _ in 0..READ_ATTEMPTS {
            if let Ok(v) = self.bus.get_vcp(self.req.monitor, VCP_INPUT_SOURCE) {
                // Algunos monitores ponen otra cosa en el byte alto; la entrada va en el bajo.
                value = Some((v.current & 0xFF) as u8).filter(|c| *c != 0);
                break;
            }
        }
        reported.set(input, value);
        self.emit(CalibrationEvent::Reported { input, value });
    }
}
