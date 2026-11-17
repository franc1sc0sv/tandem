//! Los mensajes del protocolo.
//!
//! Hay tres canales y todos empiezan con un [`Hello`]:
//! - agente ↔ agente: [`ControlMsg`] en los dos sentidos;
//! - agente ↔ agente antes de confiar el uno en el otro: [`PairingMsg`], solo por la
//!   conexión de emparejamiento;
//! - UI → agente: [`UiRequest`]; agente → UI: [`AgentToUi`].

use serde::{Deserialize, Serialize};
use tandem_core::{Config, Machine, MachineInfo, State, Step};

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
    /// Las pantallas que lee quien lo manda (las que usa si el puesto no declara las
    /// suyas), para la geometría del teclado y mouse.
    Screens(Vec<ScreenRect>),
    /// Lo que tiene y sabe hacer quien lo manda (T-084). Va al conectarse por cada
    /// enlace nuevo y cada vez que cambia; las decisiones salen de aquí, no del nombre
    /// de la máquina (DESIGN §3.7).
    Capabilities(Capabilities),
    /// El puesto compartido de quien lo manda (el acomodo de pantallas y el atajo de
    /// bloqueo), con su versión. Va al conectarse y con cada cambio; gana la versión más
    /// nueva (T-084).
    Desk(DeskSync),
    /// «Presiona una tecla» del asistente (T-087): armar o desarmar la escucha, o avisar
    /// que aquí se oyó la tecla.
    KeyProbe(KeyProbeMsg),
    /// El usuario dijo en el asistente dónde está el teclado y el mouse.
    Keyboard(Machine),
}

/// Los mensajes de la prueba «presiona una tecla» entre los dos agentes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeyProbeMsg {
    Arm,
    Disarm,
    /// La tecla llegó a la máquina que lo manda.
    Heard,
}

/// Lo que una máquina tiene y sabe hacer, tal como lo anuncia su agente.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    pub machine: Machine,
    /// Su sistema, su nombre y su color (DECISIONS A1): la UI y las decisiones que dependen
    /// del sistema salen de aquí, no del lado.
    pub info: MachineInfo,
    /// El hub del teclado y mouse está conectado aquí; `None` sin detección.
    pub hub: Option<bool>,
    pub input: InputCaps,
    pub power: PowerCaps,
    /// Servicios del plano de datos que ofrece (SSH, carpetas...); vacío hasta v2.
    pub services: Vec<String>,
}

/// El motor de teclado y mouse de una máquina.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputCaps {
    /// Puede capturar el teclado y el mouse (tenerlos y mandarlos).
    pub capture: bool,
    /// Puede inyectarlos (recibirlos de la otra).
    pub inject: bool,
    /// Permisos de macOS que le faltan.
    pub missing: Vec<InputPermission>,
    /// El proceso que corre el agente: macOS concede los permisos a ese binario exacto, y
    /// la UI le dice al usuario cuál es sin pasar por una terminal (T-090).
    pub process: ProcessInfo,
}

/// Qué proceso es el agente.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessInfo {
    /// Ruta del ejecutable en marcha (`/Applications/Tandem.app/Contents/MacOS/tandemd`);
    /// vacía si el sistema no la dio.
    pub exe: String,
    pub pid: u32,
    /// Cuándo arrancó, en segundos desde la época Unix: tras reiniciar el agente cambia, y
    /// así la UI sabe que los permisos recién dados ya se evalúan en un proceso nuevo.
    pub started: u64,
}

/// Energía de una máquina.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PowerCaps {
    /// Puede despertar a la otra (tiene su `[power.wol]`).
    pub wake_peer: bool,
    /// Se la puede despertar por Wake-on-LAN.
    pub wakeable: bool,
    /// Sabe apagarse limpio a pedido de la otra.
    pub power_off: bool,
}

/// El puesto compartido (el acomodo de pantallas y el atajo de bloqueo) con su reloj
/// lógico. Lo solo-local (identidad, puerto, rutas, máquina) no viaja.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeskSync {
    /// Versión: cada cambio sube la más alta conocida en uno.
    pub rev: u64,
    /// Quién hizo el último cambio: desempata dos versiones iguales.
    pub origin: Machine,
    pub desk: Config,
    /// El atajo que bloquea el cursor (`[input] lock_shortcut`).
    pub lock_shortcut: Vec<String>,
}

/// Lo que sabe cada máquina del puesto, para la UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeskCapabilities {
    /// Esta máquina.
    pub here: Capabilities,
    /// La otra, según su último anuncio; `None` si nunca se la vio.
    pub peer: Option<Capabilities>,
}

/// Los streams propios de la entrada y del portapapeles, aparte del de pedidos: una
/// imagen grande no frena las teclas, y cada uno tiene sus propios mensajes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Channel {
    /// La sesión de teclado y mouse: la abre el servidor.
    Input,
    /// El portapapeles: lo abre el lado A.
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

/// Una pantalla, en las coordenadas nativas del cursor de su máquina.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScreenRect {
    /// Su id en el acomodo (`mac-1`, o el de un `[[screens]]`).
    pub id: String,
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
    /// El puesto compartido (el acomodo de pantallas declarado).
    GetDesk,
    /// Encender o apagar `machine` (la otra, normalmente); apagar es la transacción de
    /// apagado limpio. Solo si alguna máquina puede (ver [`Capabilities::power`]).
    SetPower {
        machine: Machine,
        on: bool,
    },
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
    /// Pedir a macOS los permisos que falten (diálogo del sistema), a nombre de este
    /// agente. Solo por orden explícita del usuario; el estado nuevo llega con las
    /// capacidades (T-090).
    RequestPermissions,
    /// De dónde salió la configuración de este agente y en qué puerto escucha.
    GetSetup,
    /// Lo que puede cada máquina ([`DeskCapabilities`]).
    GetCapabilities,
    /// «Presiona una tecla»: las dos máquinas escuchan (o dejan de escuchar).
    KeyProbe {
        on: bool,
    },
    /// La ventana de esta máquina oyó una tecla.
    HeardKey,
    /// El usuario confirma dónde está el teclado y el mouse. Se guarda y se le dice a la
    /// otra máquina.
    SetKeyboard(Machine),
    /// Latencia visible del enlace con la otra máquina.
    PingPeer,
}

/// La configuración del agente, para que la UI la muestre.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupInfo {
    /// `tandem.toml`; `None` si el agente no tiene archivo (pruebas).
    pub config_file: Option<String>,
    /// El agente la generó al arrancar por primera vez (T-082).
    pub generated: bool,
    /// El puerto UDP real del agente; `None` sin red.
    pub port: Option<u16>,
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
    Power(PowerEvent),
    /// Aviso espontáneo cuando cambia el par o el enlace con él.
    Peer(PeerStatus),
    /// Aviso espontáneo con cada paso del emparejamiento.
    Pairing(PairingStatus),
    /// Aviso espontáneo cuando cambia el teclado y mouse (roles, sesión, permisos).
    Input(InputStatus),
    /// Aviso espontáneo cuando cambia el puesto compartido.
    Desk(Config),
    /// Aviso espontáneo cuando cambia lo que puede alguna de las dos máquinas.
    Capabilities(Box<DeskCapabilities>),
    /// Avance de la prueba «presiona una tecla» del asistente (T-087).
    KeyProbe(KeyProbeStatus),
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
    /// Se sabe dónde está el teclado: lo dice el hub, la configuración o el asistente.
    /// Si no, `server` es solo el valor con el que arranca el motor y la UI dice «sin
    /// definir»: nunca se supone la Mac (T-087).
    pub defined: bool,
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
            defined: false,
        }
    }
}

/// La prueba «presiona una tecla» vista desde la UI (T-087).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyProbeStatus {
    /// Las dos máquinas están escuchando.
    pub armed: bool,
    /// La máquina que recibió la tecla, si ya llegó una.
    pub heard: Option<Machine>,
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

/// Cambios de energía de la otra máquina que no vienen de una transacción.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PowerEvent {
    /// La otra máquina dejó de contestar por todos los enlaces sin que nadie la apagara
    /// (un corte de corriente, o se suspendió): este agente la da por apagada.
    PeerLost {
        machine: Machine,
        /// Texto para mostrar en la UI.
        reason: String,
    },
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
    Setup(SetupInfo),
    Capabilities(Box<DeskCapabilities>),
    /// Lo que tarda un ping por el enlace de `link`.
    Pong {
        link: Link,
        millis: u32,
    },
}

/// Avance de una transacción, para que la UI muestre cada paso.
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
    Committed {
        txn: u64,
        state: State,
    },
    /// Falló un paso: `state` tiene lo que sí quedó hecho (los pasos de energía no se
    /// deshacen).
    Failed {
        txn: u64,
        state: State,
    },
}

#[cfg(test)]
pub(crate) mod samples {
    //! Un ejemplo de cada variante. Las funciones `arm` hacen `match` exhaustivo: una
    //! variante nueva no compila hasta tener número, y la prueba de cobertura falla
    //! hasta que tenga ejemplo.

    use tandem_core::{Arrangement, Scale, ScreenSpec, Side};

    use super::*;

    pub fn state() -> State {
        State {
            on: tandem_core::Powered::both(),
        }
    }

    /// Un puesto con pantallas declaradas de las dos máquinas.
    pub fn desk() -> Config {
        Config {
            machines: tandem_core::Machines {
                a: Some(MachineInfo::new(Machine::A, tandem_core::Os::MacOs, None)),
                b: Some(MachineInfo::new(
                    Machine::B,
                    tandem_core::Os::Windows,
                    Some("ESCRITORIO".into()),
                )),
            },
            screens: vec![
                ScreenSpec {
                    machine: Machine::B,
                    id: "34".into(),
                    x: 0,
                    y: 0,
                    width: 3440,
                    height: 1440,
                    scale: Scale::ONE,
                },
                ScreenSpec {
                    machine: Machine::A,
                    id: "laptop".into(),
                    x: 0,
                    y: 0,
                    width: 1512,
                    height: 982,
                    scale: Scale::from_milli(2000),
                },
            ],
            arrangement: Arrangement {
                a_side: Side::Right,
                offset: -40,
            },
        }
    }

    pub fn steps() -> Vec<Step> {
        vec![
            Step::PowerOn {
                machine: Machine::B,
            },
            Step::PowerOff {
                machine: Machine::A,
            },
        ]
    }

    pub fn step_arm(s: &Step) -> usize {
        match s {
            Step::PowerOn { .. } => 0,
            Step::PowerOff { .. } => 1,
        }
    }

    pub fn hellos() -> Vec<Hello> {
        vec![
            Hello::new(Peer::Agent(Machine::A)),
            Hello::new(Peer::Agent(Machine::B)),
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
                outcome: StepOutcome::Failed("no hay Wake-on-LAN".into()),
            },
            ControlMsg::GetState,
            ControlMsg::State(state()),
            ControlMsg::Open(Channel::Input),
            ControlMsg::Open(Channel::Clipboard),
            ControlMsg::Hub(HubReport {
                hub: true,
                server: Machine::B,
            }),
            ControlMsg::Screens(vec![]),
            ControlMsg::Screens(screens()),
            ControlMsg::KeyProbe(KeyProbeMsg::Arm),
            ControlMsg::KeyProbe(KeyProbeMsg::Disarm),
            ControlMsg::KeyProbe(KeyProbeMsg::Heard),
            ControlMsg::Keyboard(Machine::B),
            ControlMsg::Desk(DeskSync {
                rev: 4,
                origin: Machine::B,
                desk: desk(),
                lock_shortcut: vec!["ScrollLock".into()],
            }),
            ControlMsg::Desk(DeskSync {
                rev: 0,
                origin: Machine::A,
                desk: Config::default(),
                lock_shortcut: vec![],
            }),
        ];
        v.extend(capabilities().into_iter().map(ControlMsg::Capabilities));
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
            ControlMsg::Capabilities(_) => 9,
            ControlMsg::Desk(_) => 10,
            ControlMsg::KeyProbe(_) => 11,
            ControlMsg::Keyboard(_) => 12,
        }
    }

    /// Las dos máquinas del puesto real.
    pub fn capabilities() -> Vec<Capabilities> {
        vec![
            Capabilities {
                machine: Machine::B,
                info: MachineInfo {
                    os: tandem_core::Os::Windows,
                    name: "ESCRITORIO".into(),
                    color: "#a99be8".into(),
                },
                hub: Some(true),
                input: InputCaps {
                    capture: true,
                    inject: true,
                    missing: vec![],
                    process: ProcessInfo::default(),
                },
                power: PowerCaps {
                    wake_peer: false,
                    wakeable: true,
                    power_off: true,
                },
                services: vec!["ssh".into()],
            },
            Capabilities {
                machine: Machine::A,
                info: MachineInfo {
                    os: tandem_core::Os::MacOs,
                    name: "MacBook Pro".into(),
                    color: "#7aa7e6".into(),
                },
                hub: None,
                input: InputCaps {
                    capture: false,
                    inject: false,
                    missing: vec![
                        InputPermission::Accessibility,
                        InputPermission::InputMonitoring,
                    ],
                    process: ProcessInfo {
                        exe: "/Applications/Tandem.app/Contents/MacOS/tandemd".into(),
                        pid: 4242,
                        started: 1_760_000_000,
                    },
                },
                power: PowerCaps {
                    wake_peer: true,
                    wakeable: false,
                    power_off: false,
                },
                services: vec![],
            },
        ]
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
                id: "pc-1".into(),
                x: -3440,
                y: 0,
                width: 3440,
                height: 1440,
                scale_milli: 1000,
            },
            ScreenRect {
                id: "laptop".into(),
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
            InputStatus::new(Machine::A),
            InputStatus {
                server: Machine::B,
                hub_here: Some(true),
                session: true,
                locked: true,
                missing: vec![
                    InputPermission::Accessibility,
                    InputPermission::InputMonitoring,
                ],
                problem: Some("falta el permiso de Accesibilidad".into()),
                defined: true,
            },
        ]
    }

    pub fn ui_requests() -> Vec<UiRequest> {
        let cmds = vec![
            UiCommand::GetState,
            UiCommand::GetDesk,
            UiCommand::SetPower {
                machine: Machine::B,
                on: true,
            },
            UiCommand::SetPower {
                machine: Machine::A,
                on: false,
            },
            UiCommand::GetPeer,
            UiCommand::GetPairing,
            UiCommand::StartPairing,
            UiCommand::ConfirmPairing,
            UiCommand::CancelPairing,
            UiCommand::Unpair,
            UiCommand::GetInput,
            UiCommand::RequestPermissions,
            UiCommand::GetSetup,
            UiCommand::GetCapabilities,
            UiCommand::KeyProbe { on: true },
            UiCommand::KeyProbe { on: false },
            UiCommand::HeardKey,
            UiCommand::SetKeyboard(Machine::B),
            UiCommand::PingPeer,
        ];
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
            UiCommand::GetDesk => 1,
            UiCommand::SetPower { .. } => 2,
            UiCommand::GetPeer => 3,
            UiCommand::GetPairing => 4,
            UiCommand::StartPairing => 5,
            UiCommand::ConfirmPairing => 6,
            UiCommand::CancelPairing => 7,
            UiCommand::Unpair => 8,
            UiCommand::GetInput => 9,
            UiCommand::RequestPermissions => 10,
            UiCommand::GetSetup => 11,
            UiCommand::GetCapabilities => 12,
            UiCommand::KeyProbe { .. } => 13,
            UiCommand::HeardKey => 14,
            UiCommand::SetKeyboard(_) => 15,
            UiCommand::PingPeer => 16,
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
                index: 1,
                error: "timeout".into(),
            },
            TxnEvent::Committed {
                txn: 1,
                state: state(),
            },
            TxnEvent::Failed {
                txn: 1,
                state: state(),
            },
        ]
    }

    pub fn txn_arm(e: &TxnEvent) -> usize {
        match e {
            TxnEvent::Started { .. } => 0,
            TxnEvent::StepStarted { .. } => 1,
            TxnEvent::StepDone { .. } => 2,
            TxnEvent::StepFailed { .. } => 3,
            TxnEvent::Committed { .. } => 4,
            TxnEvent::Failed { .. } => 5,
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
            AgentToUi::Power(PowerEvent::PeerLost {
                machine: Machine::B,
                reason: "la PC no responde".into(),
            }),
            AgentToUi::Power(PowerEvent::PeerLost {
                machine: Machine::A,
                reason: String::new(),
            }),
            AgentToUi::Reply {
                id: 7,
                reply: Reply::Input(InputStatus::new(Machine::B)),
            },
        ];
        v.push(AgentToUi::Reply {
            id: 7,
            reply: Reply::Pairing(PairingStatus::Searching),
        });
        v.extend(pairing_statuses().into_iter().map(AgentToUi::Pairing));
        v.extend(input_status().into_iter().map(AgentToUi::Input));
        v.extend(txn_events().into_iter().map(AgentToUi::Txn));
        v.push(AgentToUi::Reply {
            id: 9,
            reply: Reply::Setup(SetupInfo {
                config_file: Some(r"C:\Users\Ana\AppData\Roaming\Tandem\tandem.toml".into()),
                generated: true,
                port: Some(52100),
            }),
        });
        v.push(AgentToUi::Reply {
            id: 10,
            reply: Reply::Setup(SetupInfo {
                config_file: None,
                generated: false,
                port: None,
            }),
        });
        v.push(AgentToUi::Desk(desk()));
        v.push(AgentToUi::KeyProbe(KeyProbeStatus::default()));
        v.push(AgentToUi::KeyProbe(KeyProbeStatus {
            armed: true,
            heard: Some(Machine::B),
        }));
        v.push(AgentToUi::Reply {
            id: 13,
            reply: Reply::Pong {
                link: Link::Wifi,
                millis: 7,
            },
        });
        let [pc, mac] = <[Capabilities; 2]>::try_from(capabilities()).unwrap();
        v.push(AgentToUi::Capabilities(Box::new(DeskCapabilities {
            here: mac.clone(),
            peer: Some(pc),
        })));
        v.push(AgentToUi::Reply {
            id: 11,
            reply: Reply::Capabilities(Box::new(DeskCapabilities {
                here: mac,
                peer: None,
            })),
        });
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
            AgentToUi::Power(_) => 3,
            AgentToUi::Peer(_) => 4,
            AgentToUi::Pairing(_) => 5,
            AgentToUi::Input(_) => 6,
            AgentToUi::Desk(_) => 7,
            AgentToUi::Capabilities(_) => 8,
            AgentToUi::KeyProbe(_) => 9,
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
            Reply::Setup(_) => 8,
            Reply::Capabilities(_) => 9,
            Reply::Pong { .. } => 10,
        }
    }

    pub fn peer_arm(p: &Peer) -> usize {
        match p {
            Peer::Agent(Machine::A) => 0,
            Peer::Agent(Machine::B) => 1,
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
        covers(&steps(), step_arm, 2);
        covers(&hellos(), |h| peer_arm(&h.peer), 3);
        covers(&control(), control_arm, 13);
        let cmds: Vec<_> = ui_requests().into_iter().map(|r| r.command).collect();
        covers(&cmds, ui_command_arm, 17);
        covers(&to_ui(), to_ui_arm, 10);
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
        covers(&replies, reply_arm, 11);
        covers(&txn_events(), txn_arm, 6);
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

    /// Un agente de antes de los lados A/B (v15, con `Peer::Agent(Mac)`) se corta en el
    /// saludo, con un mensaje que dice qué hacer.
    #[test]
    fn an_agent_from_before_the_sides_is_rejected_in_the_hello() {
        let old = crate::encode(&(15u16, 0u8, 0u8)).unwrap();
        let e = crate::decode_hello(&old).unwrap_err();
        assert_eq!(
            e,
            ProtoError::IncompatibleVersion {
                ours: PROTOCOL_VERSION,
                theirs: 15,
            }
        );
        let text = e.to_string();
        assert!(text.contains("lados A/B"), "{text}");
        assert!(text.contains("misma versión"), "{text}");
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
