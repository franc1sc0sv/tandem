//! Los mensajes del protocolo.
//!
//! Hay tres canales y todos empiezan con un [`Hello`]:
//! - agente ↔ agente: [`ControlMsg`] en los dos sentidos;
//! - agente ↔ agente antes de confiar el uno en el otro: [`PairingMsg`], solo por la
//!   conexión de emparejamiento;
//! - UI → agente: [`UiRequest`]; agente → UI: [`AgentToUi`].

use serde::{Deserialize, Serialize};
use tandem_core::{BuiltinDisplay, Config, Input, Machine, Mode, MonitorId, State, Step};

use crate::{PROTOCOL_VERSION, ProtoError, wire};

/// Quién abre la conexión.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Peer {
    Agent(Machine),
    Ui,
}

/// Primer mensaje de toda conexión, en cada sentido.
///
/// `version` tiene que seguir siendo el **primer campo** en todas las versiones:
/// [`crate::decode_hello`] lo lee antes que el resto para poder rechazar con un
/// error claro aunque lo demás haya cambiado de forma.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub version: u16,
    pub peer: Peer,
}

impl Hello {
    pub fn new(peer: Peer) -> Self {
        Hello {
            version: PROTOCOL_VERSION,
            peer,
        }
    }

    pub fn check_version(&self) -> Result<(), ProtoError> {
        check_version(self.version)
    }
}

pub(crate) fn check_version(theirs: u16) -> Result<(), ProtoError> {
    if theirs == PROTOCOL_VERSION {
        Ok(())
    } else {
        Err(ProtoError::IncompatibleVersion {
            ours: PROTOCOL_VERSION,
            theirs,
        })
    }
}

/// Plano de control entre los dos agentes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ControlMsg {
    /// Latido; el otro lado contesta [`ControlMsg::Pong`] con el mismo `nonce`.
    Ping {
        nonce: u64,
    },
    Pong {
        nonce: u64,
    },
    /// El orquestador pide al otro agente un paso de un plan que solo él puede hacer.
    ExecuteStep {
        id: u64,
        #[serde(with = "wire::step")]
        step: Step,
    },
    /// Respuesta a [`ControlMsg::ExecuteStep`] con el mismo `id`.
    StepResult {
        id: u64,
        outcome: StepOutcome,
    },
    GetState,
    State(State),
    /// Primer mensaje de un stream que no es de pedidos: lo que sigue por él son los
    /// mensajes de ese canal, no [`ControlMsg`].
    Open(Channel),
    /// Dónde ve el hub este agente y a quién tiene de servidor de teclado y mouse.
    Hub(HubReport),
    /// Las pantallas visibles de quien lo manda, para la geometría del teclado y mouse.
    Screens(Vec<ScreenRect>),
}

/// Los streams propios de la entrada y del portapapeles, aparte del de pedidos: una
/// imagen grande no frena las teclas, y cada uno tiene sus propios mensajes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Channel {
    /// La sesión de teclado y mouse: la abre el servidor.
    Input,
    /// El portapapeles: lo abre la Mac.
    Clipboard,
}

/// Lo que un agente le reporta al otro para decidir los roles (DESIGN §7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HubReport {
    /// El hub (ya con antirrebote) está en la máquina de quien lo manda.
    pub hub: bool,
    /// Quién es el servidor para quien lo manda.
    pub server: Machine,
}

/// Una pantalla visible, en las coordenadas nativas del cursor de su máquina.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenRect {
    /// El monitor de la configuración; `None` es la pantalla integrada de la MacBook.
    pub monitor: Option<MonitorId>,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    /// Unidades nativas por punto lógico, en milésimas (1500 = 150 %): entero para que
    /// el mensaje se pueda comparar.
    pub scale_milli: u32,
}

/// Emparejamiento por comparación numérica (DESIGN §9), en el orden en que se mandan:
/// quien inicia manda `Offer`, quien responde `Answer`, quien inicia `Reveal`; después
/// cada lado manda `Confirm` cuando su usuario confirma el código, o `Reject`.
///
/// Las llaves van en bytes crudos: este crate no conoce los tipos de identidad. Quien
/// los recibe comprueba que sean las mismas que autenticó la conexión.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PairingMsg {
    Offer {
        public_key: [u8; 32],
        name: String,
        commitment: [u8; 32],
    },
    Answer {
        public_key: [u8; 32],
        name: String,
        nonce: [u8; 32],
    },
    Reveal {
        nonce: [u8; 32],
    },
    /// El usuario de este lado confirmó que ve `code`.
    Confirm {
        code: u32,
    },
    /// El usuario de este lado rechazó el código o canceló.
    Reject,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepOutcome {
    Done,
    Failed(String),
}

impl From<Result<(), String>> for StepOutcome {
    fn from(r: Result<(), String>) -> Self {
        match r {
            Ok(()) => StepOutcome::Done,
            Err(e) => StepOutcome::Failed(e),
        }
    }
}

impl From<StepOutcome> for Result<(), String> {
    fn from(o: StepOutcome) -> Self {
        match o {
            StepOutcome::Done => Ok(()),
            StepOutcome::Failed(e) => Err(e),
        }
    }
}

/// Pedido de la UI; la respuesta llega como [`AgentToUi::Reply`] con el mismo `id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UiRequest {
    pub id: u64,
    pub command: UiCommand,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum UiCommand {
    GetState,
    SetMode(#[serde(with = "wire::mode")] Mode),
    /// Arrastrar un monitor a la otra máquina.
    MoveMonitor {
        monitor: MonitorId,
        to: Machine,
    },
    /// Preferencia de la pantalla integrada de la MacBook.
    SetBuiltin(BuiltinDisplay),
    /// Respuesta a [`ManualSwitchEvent::Requested`]: `done` si el usuario ya cambió la
    /// entrada con los botones del monitor, `false` si no puede.
    AnswerManualSwitch {
        id: u64,
        done: bool,
    },
    /// El puesto configurado (monitores, nombres y cableado), para dibujar la escena.
    GetDesk,
    /// Encender (`true`) o apagar la PC; apagar es la transacción de apagado limpio.
    SetPcPower(bool),
    /// Con quién está emparejado el agente y por qué enlace lo ve.
    GetPeer,
    /// En qué va el emparejamiento.
    GetPairing,
    /// Buscar a la otra máquina para emparejarse; su avance llega en [`AgentToUi::Pairing`].
    StartPairing,
    /// El usuario ve el mismo código en las dos pantallas.
    ConfirmPairing,
    /// El usuario rechaza el código, o deja de buscar.
    CancelPairing,
    /// Olvidar al par: se borra de la confianza y deja de buscarse.
    Unpair,
    /// Quién es el servidor de teclado y mouse y si a esta máquina le faltan permisos.
    GetInput,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentToUi {
    Reply {
        id: u64,
        reply: Reply,
    },
    /// Aviso espontáneo cuando el estado cambia por cualquier motivo.
    State(State),
    Txn(TxnEvent),
    ManualSwitch(ManualSwitchEvent),
    Power(PowerEvent),
    /// Aviso espontáneo cuando cambia el par o el enlace con él.
    Peer(PeerStatus),
    /// Aviso espontáneo con cada paso del emparejamiento.
    Pairing(PairingStatus),
    /// Aviso espontáneo cuando cambia el teclado y mouse (roles, sesión, permisos).
    Input(InputStatus),
}

/// El emparejamiento visto desde la UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PairingStatus {
    /// Nadie pidió emparejar (o se canceló).
    Idle,
    /// Buscando a la otra máquina, que también tiene que estar emparejando.
    Searching,
    /// Las dos máquinas se encontraron: el usuario compara `code` en las dos pantallas.
    Comparing {
        /// Los 6 dígitos, con los ceros a la izquierda.
        code: String,
        /// El nombre con que se presentó el otro agente.
        peer: String,
        /// El usuario de esta máquina ya confirmó; falta el de la otra.
        confirmed: bool,
    },
    /// Emparejado y guardado en la confianza.
    Done { peer: String },
    /// No se emparejó; no se guardó nada.
    Failed { reason: String },
}

/// El teclado y el mouse compartidos, vistos desde este agente (el chip de la UI).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputStatus {
    /// La máquina que tiene el teclado y el mouse.
    pub server: Machine,
    /// Si el hub está en esta máquina; `None` sin detección (sin `[input.hub]` o
    /// sin escáner en este sistema).
    pub hub_here: Option<bool>,
    /// La sesión de entrada con el otro agente está establecida.
    pub session: bool,
    /// El cursor está bloqueado en la pantalla actual (atajo del servidor).
    pub locked: bool,
    /// Permisos del sistema que faltan para capturar o inyectar (macOS). Vacío si
    /// no falta ninguno.
    pub missing: Vec<InputPermission>,
    /// Por qué el motor de entrada de esta máquina no funciona, si no funciona.
    pub problem: Option<String>,
}

impl InputStatus {
    /// Antes de saber nada: el servidor con el que arranca el agente.
    pub fn new(server: Machine) -> Self {
        InputStatus {
            server,
            hub_here: None,
            session: false,
            locked: false,
            missing: Vec::new(),
            problem: None,
        }
    }
}

/// Permisos de macOS (TCC) que necesita el motor de entrada.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputPermission {
    /// Accesibilidad: capturar tragando eventos e inyectar.
    Accessibility,
    /// Monitorización de entrada: ver el teclado.
    InputMonitoring,
}

/// El otro agente visto desde este.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PeerStatus {
    /// Sin par emparejado: el agente trabaja solo, con lo que esta máquina puede hacer.
    Unpaired,
    Paired {
        /// El nombre con que se guardó en la confianza.
        name: String,
        /// El enlace por el que van los pedidos; `None` si ninguno ve al otro agente.
        link: Option<Link>,
    },
}

/// Los enlaces entre las dos máquinas, de mayor a menor prioridad (DESIGN §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Link {
    Usb4,
    Ethernet,
    Wifi,
}

/// Cambios de energía de la PC que no vienen de un cambio de modo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PowerEvent {
    /// La PC dejó de contestar por todos los enlaces sin que nadie la apagara (lo más
    /// probable, un corte de corriente): el agente de la Mac la da por apagada.
    PcLost {
        /// Texto para mostrar en la UI.
        reason: String,
        /// Monitores que siguen en la entrada de la PC y quedan en negro hasta
        /// encenderla: la Mac no puede cambiarles la entrada sin ella.
        stranded: Vec<MonitorId>,
    },
}

/// Un monitor sin DDC: el usuario tiene que cambiar la entrada a mano y confirmarlo con
/// [`UiCommand::AnswerManualSwitch`] antes de que venza el paso.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ManualSwitchEvent {
    Requested {
        id: u64,
        monitor: MonitorId,
        label: String,
        /// La máquina que lo va a mostrar.
        to: Machine,
        input: Input,
    },
    /// Ya no espera respuesta (contestado, vencido o cancelado).
    Closed { id: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Reply {
    State(State),
    /// El cambio arrancó como la transacción `txn`; su avance llega en [`AgentToUi::Txn`].
    Accepted {
        txn: u64,
    },
    Rejected(String),
    /// Pedido atendido, sin nada más que contestar.
    Done,
    Desk(Config),
    Peer(PeerStatus),
    Pairing(PairingStatus),
    Input(InputStatus),
}

/// Avance de una transacción, para que la UI muestre cada paso y nunca una pantalla
/// negra sin explicación.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TxnEvent {
    Started {
        txn: u64,
        #[serde(with = "wire::steps")]
        steps: Vec<Step>,
    },
    /// `index` es la posición en `steps` de [`TxnEvent::Started`].
    StepStarted {
        txn: u64,
        index: u32,
    },
    StepDone {
        txn: u64,
        index: u32,
    },
    StepFailed {
        txn: u64,
        index: u32,
        error: String,
    },
    RollingBack {
        txn: u64,
    },
    Committed {
        txn: u64,
        state: State,
    },
    /// Falló y se deshizo todo: `state` es el de partida.
    RolledBack {
        txn: u64,
        state: State,
    },
    /// Falló y no se pudo deshacer todo: el estado real es incierto.
    Inconsistent {
        txn: u64,
        undo_failures: Vec<UndoFailure>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UndoFailure {
    #[serde(with = "wire::step")]
    pub step: Step,
    pub error: String,
}

#[cfg(test)]
pub(crate) mod samples {
    //! Un ejemplo de cada variante. Las funciones `arm` hacen `match` exhaustivo: una
    //! variante nueva no compila hasta tener número, y la prueba de cobertura falla
    //! hasta que tenga ejemplo.

    use std::collections::BTreeMap;

    use super::*;

    pub fn state() -> State {
        State {
            owner: BTreeMap::from([("m34".into(), Machine::Pc), ("m32".into(), Machine::Mac)]),
            pc_on: true,
            builtin_pref: BuiltinDisplay::Mirror,
        }
    }

    /// Un puesto con todo lo opcional presente y ausente.
    pub fn desk() -> Config {
        Config::from_toml(
            r#"
            [[monitors]]
            id = "m34"
            label = '34"'
            edid_model = "LC34G55T"
            wiring = { pc = "displayport", mac = "hdmi" }
            ddc = { hdmi = 0x06, display_port = 0x0F }

            [[monitors]]
            id = "m32"
            label = '32"'
            wiring = { mac = "hdmi" }
            "#,
        )
        .unwrap()
    }

    pub fn steps() -> Vec<Step> {
        vec![
            Step::PowerOnPc,
            Step::PowerOffPc,
            Step::SwitchInput {
                monitor: "m34".into(),
                from: Machine::Mac,
                to: Machine::Pc,
                input: Input::DisplayPort,
            },
            Step::Attach {
                machine: Machine::Pc,
                monitor: "m34".into(),
            },
            Step::Detach {
                machine: Machine::Mac,
                monitor: "m34".into(),
            },
            Step::SetBuiltin {
                from: BuiltinDisplay::Off,
                to: BuiltinDisplay::Extend,
            },
            Step::UpdateInputGeometry,
        ]
    }

    pub fn step_arm(s: &Step) -> usize {
        match s {
            Step::PowerOnPc => 0,
            Step::PowerOffPc => 1,
            Step::SwitchInput { .. } => 2,
            Step::Attach { .. } => 3,
            Step::Detach { .. } => 4,
            Step::SetBuiltin { .. } => 5,
            Step::UpdateInputGeometry => 6,
        }
    }

    pub fn modes() -> Vec<Mode> {
        vec![
            Mode::SoloMac,
            Mode::Shared(BTreeMap::from([("m34".into(), Machine::Pc)])),
            Mode::Shared(BTreeMap::new()),
            Mode::SoloPc,
        ]
    }

    pub fn mode_arm(m: &Mode) -> usize {
        match m {
            Mode::SoloMac => 0,
            Mode::Shared(_) => 1,
            Mode::SoloPc => 2,
        }
    }

    pub fn hellos() -> Vec<Hello> {
        vec![
            Hello::new(Peer::Agent(Machine::Mac)),
            Hello::new(Peer::Agent(Machine::Pc)),
            Hello::new(Peer::Ui),
        ]
    }

    pub fn control() -> Vec<ControlMsg> {
        let mut v = vec![
            ControlMsg::Ping { nonce: 0 },
            ControlMsg::Pong { nonce: u64::MAX },
            ControlMsg::StepResult {
                id: 7,
                outcome: StepOutcome::Done,
            },
            ControlMsg::StepResult {
                id: 8,
                outcome: StepOutcome::Failed("DDC no responde".into()),
            },
            ControlMsg::GetState,
            ControlMsg::State(state()),
            ControlMsg::Open(Channel::Input),
            ControlMsg::Open(Channel::Clipboard),
            ControlMsg::Hub(HubReport {
                hub: true,
                server: Machine::Pc,
            }),
            ControlMsg::Screens(vec![]),
            ControlMsg::Screens(screens()),
        ];
        v.extend(
            steps()
                .into_iter()
                .enumerate()
                .map(|(i, step)| ControlMsg::ExecuteStep { id: i as u64, step }),
        );
        v
    }

    pub fn control_arm(m: &ControlMsg) -> usize {
        match m {
            ControlMsg::Ping { .. } => 0,
            ControlMsg::Pong { .. } => 1,
            ControlMsg::ExecuteStep { .. } => 2,
            ControlMsg::StepResult { .. } => 3,
            ControlMsg::GetState => 4,
            ControlMsg::State(_) => 5,
            ControlMsg::Open(_) => 6,
            ControlMsg::Hub(_) => 7,
            ControlMsg::Screens(_) => 8,
        }
    }

    pub fn pairing() -> Vec<PairingMsg> {
        vec![
            PairingMsg::Offer {
                public_key: [1; 32],
                name: "Mac".into(),
                commitment: [2; 32],
            },
            PairingMsg::Answer {
                public_key: [3; 32],
                name: "PC".into(),
                nonce: [4; 32],
            },
            PairingMsg::Reveal { nonce: [5; 32] },
            PairingMsg::Confirm { code: 42 },
            PairingMsg::Reject,
        ]
    }

    pub fn pairing_arm(m: &PairingMsg) -> usize {
        match m {
            PairingMsg::Offer { .. } => 0,
            PairingMsg::Answer { .. } => 1,
            PairingMsg::Reveal { .. } => 2,
            PairingMsg::Confirm { .. } => 3,
            PairingMsg::Reject => 4,
        }
    }

    pub fn pairing_statuses() -> Vec<PairingStatus> {
        vec![
            PairingStatus::Idle,
            PairingStatus::Searching,
            PairingStatus::Comparing {
                code: "004217".into(),
                peer: "PC".into(),
                confirmed: false,
            },
            PairingStatus::Done { peer: "PC".into() },
            PairingStatus::Failed {
                reason: "la otra máquina rechazó el código".into(),
            },
        ]
    }

    pub fn screens() -> Vec<ScreenRect> {
        vec![
            ScreenRect {
                monitor: Some("m34".into()),
                x: -3440,
                y: 0,
                width: 3440,
                height: 1440,
                scale_milli: 1000,
            },
            ScreenRect {
                monitor: None,
                x: 0,
                y: 0,
                width: 1512,
                height: 982,
                scale_milli: 2000,
            },
        ]
    }

    pub fn pairing_status_arm(s: &PairingStatus) -> usize {
        match s {
            PairingStatus::Idle => 0,
            PairingStatus::Searching => 1,
            PairingStatus::Comparing { .. } => 2,
            PairingStatus::Done { .. } => 3,
            PairingStatus::Failed { .. } => 4,
        }
    }

    pub fn input_status() -> Vec<InputStatus> {
        vec![
            InputStatus::new(Machine::Mac),
            InputStatus {
                server: Machine::Pc,
                hub_here: Some(true),
                session: true,
                locked: true,
                missing: vec![
                    InputPermission::Accessibility,
                    InputPermission::InputMonitoring,
                ],
                problem: Some("falta el permiso de Accesibilidad".into()),
            },
        ]
    }

    pub fn ui_requests() -> Vec<UiRequest> {
        let mut cmds = vec![
            UiCommand::GetState,
            UiCommand::MoveMonitor {
                monitor: "m32".into(),
                to: Machine::Pc,
            },
            UiCommand::SetBuiltin(BuiltinDisplay::Off),
            UiCommand::SetBuiltin(BuiltinDisplay::Mirror),
            UiCommand::SetBuiltin(BuiltinDisplay::Extend),
            UiCommand::AnswerManualSwitch { id: 3, done: true },
            UiCommand::AnswerManualSwitch { id: 4, done: false },
            UiCommand::GetDesk,
            UiCommand::SetPcPower(true),
            UiCommand::SetPcPower(false),
            UiCommand::GetPeer,
            UiCommand::GetPairing,
            UiCommand::StartPairing,
            UiCommand::ConfirmPairing,
            UiCommand::CancelPairing,
            UiCommand::Unpair,
            UiCommand::GetInput,
        ];
        cmds.extend(modes().into_iter().map(UiCommand::SetMode));
        cmds.into_iter()
            .enumerate()
            .map(|(i, command)| UiRequest {
                id: i as u64,
                command,
            })
            .collect()
    }

    pub fn ui_command_arm(c: &UiCommand) -> usize {
        match c {
            UiCommand::GetState => 0,
            UiCommand::SetMode(_) => 1,
            UiCommand::MoveMonitor { .. } => 2,
            UiCommand::SetBuiltin(_) => 3,
            UiCommand::AnswerManualSwitch { .. } => 4,
            UiCommand::GetDesk => 5,
            UiCommand::SetPcPower(_) => 6,
            UiCommand::GetPeer => 7,
            UiCommand::GetPairing => 8,
            UiCommand::StartPairing => 9,
            UiCommand::ConfirmPairing => 10,
            UiCommand::CancelPairing => 11,
            UiCommand::Unpair => 12,
            UiCommand::GetInput => 13,
        }
    }

    pub fn txn_events() -> Vec<TxnEvent> {
        vec![
            TxnEvent::Started {
                txn: 1,
                steps: steps(),
            },
            TxnEvent::Started {
                txn: 2,
                steps: vec![],
            },
            TxnEvent::StepStarted { txn: 1, index: 0 },
            TxnEvent::StepDone { txn: 1, index: 0 },
            TxnEvent::StepFailed {
                txn: 1,
                index: 2,
                error: "timeout".into(),
            },
            TxnEvent::RollingBack { txn: 1 },
            TxnEvent::Committed {
                txn: 1,
                state: state(),
            },
            TxnEvent::RolledBack {
                txn: 1,
                state: state(),
            },
            TxnEvent::Inconsistent {
                txn: 1,
                undo_failures: steps()
                    .into_iter()
                    .map(|step| UndoFailure {
                        step,
                        error: "sin respuesta".into(),
                    })
                    .collect(),
            },
        ]
    }

    pub fn txn_arm(e: &TxnEvent) -> usize {
        match e {
            TxnEvent::Started { .. } => 0,
            TxnEvent::StepStarted { .. } => 1,
            TxnEvent::StepDone { .. } => 2,
            TxnEvent::StepFailed { .. } => 3,
            TxnEvent::RollingBack { .. } => 4,
            TxnEvent::Committed { .. } => 5,
            TxnEvent::RolledBack { .. } => 6,
            TxnEvent::Inconsistent { .. } => 7,
        }
    }

    pub fn to_ui() -> Vec<AgentToUi> {
        let mut v = vec![
            AgentToUi::Reply {
                id: 1,
                reply: Reply::State(state()),
            },
            AgentToUi::Reply {
                id: 2,
                reply: Reply::Accepted { txn: 9 },
            },
            AgentToUi::Reply {
                id: 3,
                reply: Reply::Rejected("la PC no responde".into()),
            },
            AgentToUi::Reply {
                id: 4,
                reply: Reply::Done,
            },
            AgentToUi::Reply {
                id: 5,
                reply: Reply::Desk(desk()),
            },
            AgentToUi::Reply {
                id: 6,
                reply: Reply::Peer(PeerStatus::Unpaired),
            },
            AgentToUi::Peer(PeerStatus::Paired {
                name: "PC".into(),
                link: None,
            }),
            AgentToUi::State(state()),
            AgentToUi::ManualSwitch(ManualSwitchEvent::Requested {
                id: 1,
                monitor: "m32".into(),
                label: "32\"".into(),
                to: Machine::Pc,
                input: Input::DisplayPort,
            }),
            AgentToUi::ManualSwitch(ManualSwitchEvent::Closed { id: 1 }),
            AgentToUi::Power(PowerEvent::PcLost {
                reason: "la PC no responde".into(),
                stranded: vec!["m34".into()],
            }),
            AgentToUi::Power(PowerEvent::PcLost {
                reason: String::new(),
                stranded: vec![],
            }),
            AgentToUi::Reply {
                id: 7,
                reply: Reply::Input(InputStatus::new(Machine::Pc)),
            },
        ];
        v.push(AgentToUi::Reply {
            id: 7,
            reply: Reply::Pairing(PairingStatus::Searching),
        });
        v.extend(pairing_statuses().into_iter().map(AgentToUi::Pairing));
        v.extend(input_status().into_iter().map(AgentToUi::Input));
        v.extend(txn_events().into_iter().map(AgentToUi::Txn));
        v.extend(links().into_iter().map(|link| {
            AgentToUi::Peer(PeerStatus::Paired {
                name: "Mac".into(),
                link: Some(link),
            })
        }));
        v
    }

    pub fn links() -> Vec<Link> {
        vec![Link::Usb4, Link::Ethernet, Link::Wifi]
    }

    pub fn link_arm(l: &Link) -> usize {
        match l {
            Link::Usb4 => 0,
            Link::Ethernet => 1,
            Link::Wifi => 2,
        }
    }

    pub fn to_ui_arm(m: &AgentToUi) -> usize {
        match m {
            AgentToUi::Reply { .. } => 0,
            AgentToUi::State(_) => 1,
            AgentToUi::Txn(_) => 2,
            AgentToUi::ManualSwitch(_) => 3,
            AgentToUi::Power(_) => 4,
            AgentToUi::Peer(_) => 5,
            AgentToUi::Pairing(_) => 6,
            AgentToUi::Input(_) => 7,
        }
    }

    pub fn reply_arm(r: &Reply) -> usize {
        match r {
            Reply::State(_) => 0,
            Reply::Accepted { .. } => 1,
            Reply::Rejected(_) => 2,
            Reply::Done => 3,
            Reply::Desk(_) => 4,
            Reply::Peer(_) => 5,
            Reply::Pairing(_) => 6,
            Reply::Input(_) => 7,
        }
    }

    pub fn peer_arm(p: &Peer) -> usize {
        match p {
            Peer::Agent(Machine::Mac) => 0,
            Peer::Agent(Machine::Pc) => 1,
            Peer::Ui => 2,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fmt::Debug;

    use serde::Serialize;
    use serde::de::DeserializeOwned;

    use super::samples::*;
    use super::*;
    use crate::{decode, encode};

    fn round_trip<T: Serialize + DeserializeOwned + PartialEq + Debug>(msgs: &[T]) {
        for m in msgs {
            let bytes = encode(m).unwrap();
            assert_eq!(&decode::<T>(&bytes).unwrap(), m);
        }
    }

    /// Las muestras cubren las `n` variantes de un enum.
    fn covers<T>(samples: &[T], arm: impl Fn(&T) -> usize, n: usize) {
        let seen: BTreeSet<usize> = samples.iter().map(arm).collect();
        assert_eq!(seen, (0..n).collect(), "faltan muestras de alguna variante");
    }

    #[test]
    fn samples_cover_every_variant() {
        covers(&steps(), step_arm, 7);
        covers(&modes(), mode_arm, 3);
        covers(&hellos(), |h| peer_arm(&h.peer), 3);
        covers(&control(), control_arm, 9);
        let cmds: Vec<_> = ui_requests().into_iter().map(|r| r.command).collect();
        covers(&cmds, ui_command_arm, 14);
        covers(&to_ui(), to_ui_arm, 8);
        covers(&pairing(), pairing_arm, 5);
        covers(&pairing_statuses(), pairing_status_arm, 5);
        covers(&links(), link_arm, 3);
        let replies: Vec<_> = to_ui()
            .into_iter()
            .filter_map(|m| match m {
                AgentToUi::Reply { reply, .. } => Some(reply),
                _ => None,
            })
            .collect();
        covers(&replies, reply_arm, 8);
        covers(&txn_events(), txn_arm, 8);
    }

    #[test]
    fn hello_round_trip() {
        round_trip(&hellos());
    }

    #[test]
    fn control_round_trip() {
        round_trip(&control());
    }

    #[test]
    fn pairing_round_trip() {
        round_trip(&pairing());
    }

    #[test]
    fn ui_request_round_trip() {
        round_trip(&ui_requests());
    }

    #[test]
    fn agent_to_ui_round_trip() {
        round_trip(&to_ui());
    }

    #[test]
    fn step_outcome_converts_from_and_to_result() {
        assert_eq!(StepOutcome::from(Ok(())), StepOutcome::Done);
        let failed = StepOutcome::from(Err::<(), _>("x".to_owned()));
        assert_eq!(Result::<(), String>::from(failed), Err("x".to_owned()));
    }

    #[test]
    fn current_hello_passes_check() {
        assert_eq!(Hello::new(Peer::Ui).check_version(), Ok(()));
    }

    #[test]
    fn other_version_fails_check() {
        let h = Hello {
            version: PROTOCOL_VERSION + 1,
            peer: Peer::Ui,
        };
        assert_eq!(
            h.check_version(),
            Err(ProtoError::IncompatibleVersion {
                ours: PROTOCOL_VERSION,
                theirs: PROTOCOL_VERSION + 1,
            })
        );
    }
}
