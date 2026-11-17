//! Los mensajes del protocolo.
//!
//! Hay tres canales y todos empiezan con un [`Hello`]:
//! - agente ↔ agente: [`ControlMsg`] en los dos sentidos;
//! - agente ↔ agente antes de confiar el uno en el otro: [`PairingMsg`], solo por la
//!   conexión de emparejamiento;
//! - UI → agente: [`UiRequest`]; agente → UI: [`AgentToUi`].

use serde::{Deserialize, Serialize};
use tandem_core::{
    Config, DeskPatch, DeskVersion, HistoryEntry, Machine, MachineInfo, MonitorInput, State, Step,
};

use crate::env::{Environment, Hardware, Resources};
use crate::tunnel::{ClientKind, Project, ProjectTunnels, Tunnel, TunnelId};
use crate::{PROTOCOL_VERSION, ProtoError, wire};

/// Quién abre la conexión.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Peer {
    Agent(Machine),
    Ui,
    /// Un cliente de Claude o de terminal por la IPC local (`tandem mcp`, `tandem`): el
    /// único que puede abrir y cerrar túneles (DESIGN §11; la UI solo los mira, D9).
    Client(ClientKind),
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
    Capabilities(Box<Capabilities>),
    /// El puesto compartido de quien lo manda, con su sello. Va al conectarse y con cada
    /// cambio; gana el último (T-099, [`DeskVersion::receive`]).
    Desk(Box<DeskVersion>),
    /// «Presiona una tecla» del asistente (T-087): armar o desarmar la escucha, o avisar
    /// que aquí se oyó la tecla.
    KeyProbe(KeyProbeMsg),
    /// El usuario dijo en el asistente dónde está el teclado y el mouse.
    Keyboard(Machine),
    /// Lo que leyó el dueño de cada monitor que tiene (DECISIONS D13): su entrada real, si
    /// está encendido y los últimos eventos. Solo el dueño lee eventos; la otra máquina los
    /// conoce por aquí. Va cada vez que cambia algo y al aparecer un enlace.
    Monitors(Vec<MonitorReport>),
    /// Quien lo manda quiere los recursos en vivo de quien lo recibe (T-105). Con `on` se
    /// renueva cada pocos segundos mientras alguien mira: si deja de llegar (el enlace se
    /// cayó), el otro deja de mandarlos solo.
    WatchResources {
        on: bool,
    },
    /// Una muestra de los recursos de quien lo manda.
    Resources(Box<Resources>),
    /// Volver a escanear los entornos de quien lo recibe; lo encontrado va en su próximo
    /// [`ControlMsg::Capabilities`].
    Rescan,
    /// Actualizar las dos máquinas, una a la vez (T-122).
    Update(UpdateMsg),
    /// Quién manda el teclado y el mouse: los dos capturan y manda la máquina que
    /// recibe entrada física (DECISIONS D7, DESIGN §7).
    Focus(FocusMsg),
}

/// Lo que se dicen los dos agentes para actualizarse (T-122). Las versiones son las de la
/// app (`x.y.z` o `x.y.z-rc.n`); quien las compara es la UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum UpdateMsg {
    /// La versión que corre quien lo manda. Va al conectarse: así la otra sabe cuándo
    /// volvió con la versión nueva.
    Version(String),
    /// «Instala `version`»: el agente que lo recibe se lo pasa a su UI, que es la que
    /// descarga y reinicia.
    Install { version: String },
}

/// Las versiones de las dos máquinas, para la UI.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateStatus {
    /// La de este agente.
    pub here: String,
    /// La última que anunció la otra; `None` si nunca se la oyó.
    pub peer: Option<String>,
    /// La otra pidió que esta máquina instale esta versión.
    pub install: Option<String>,
    /// La otra se conectó hablando otra versión del protocolo (la suya): corre otra
    /// versión de Tandem, aunque no se pueda saber cuál.
    pub peer_protocol: Option<u16>,
}

/// El arbitraje del teclado y el mouse sin dueño (D7). Cada cambio sube `epoch`; con dos
/// vistas distintas gana la de `epoch` mayor y, empatadas, la del lado `A`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FocusMsg {
    /// Quien lo manda recibe entrada física y no es quien manda: la pide.
    Claim { epoch: u64 },
    /// Quien lo manda deja de mandar: desde `epoch` manda el que lo recibe.
    Grant { epoch: u64 },
    /// Quién manda según quien lo manda (al conectar, cada tanto y ante un pedido viejo).
    State { epoch: u64, server: Machine },
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
    /// Los módulos que existen en su sistema, con su estado (DECISIONS D3, T-098). Los
    /// que no existen allí no aparecen.
    pub modules: Vec<ModuleInfo>,
    /// Entornos de ejecución detectados (T-105); vacío hasta que termina el primer escaneo.
    pub environments: Vec<Environment>,
    /// El equipo según el escaneo (T-105).
    pub hardware: Hardware,
    /// Monitores con canal DDC vivo desde esta máquina. Vacío hasta T-110.
    pub monitors: Vec<MonitorChannel>,
}

/// Los módulos de Tandem. Cada máquina anuncia solo los que existen en su sistema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ModuleId {
    /// Teclado y mouse compartidos.
    Input,
    Clipboard,
    /// Arrastrar archivos entre máquinas (D14).
    Files,
    /// Túneles de Claude (D8).
    Tunnels,
    /// Control de monitores por DDC (D11).
    Monitors,
    /// Wake-on-LAN y apagado limpio.
    Power,
    /// Entrada en el escritorio seguro de Windows (UAC, inicio de sesión; T-027).
    SecureDesktop,
}

impl ModuleId {
    /// Como se guarda en el puesto (`[modules.a] clipboard = false`): estable, no cambia
    /// aunque cambie el nombre que muestra la UI.
    pub fn as_str(self) -> &'static str {
        match self {
            ModuleId::Input => "input",
            ModuleId::Clipboard => "clipboard",
            ModuleId::Files => "files",
            ModuleId::Tunnels => "tunnels",
            ModuleId::Monitors => "monitors",
            ModuleId::Power => "power",
            ModuleId::SecureDesktop => "secure_desktop",
        }
    }

    /// Lo inverso de [`ModuleId::as_str`].
    pub fn parse(s: &str) -> Option<ModuleId> {
        ModuleId::ALL.into_iter().find(|m| m.as_str() == s)
    }

    pub const ALL: [ModuleId; 7] = [
        ModuleId::Input,
        ModuleId::Clipboard,
        ModuleId::Files,
        ModuleId::Tunnels,
        ModuleId::Monitors,
        ModuleId::Power,
        ModuleId::SecureDesktop,
    ];
}

/// En qué está un módulo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModuleState {
    Active,
    /// Lo apagó el usuario.
    Off,
    /// No puede funcionar; `reason` dice por qué y, si se puede, qué hacer.
    Unavailable {
        reason: String,
    },
}

/// Un módulo tal como lo anuncia su máquina.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleInfo {
    pub id: ModuleId,
    /// Para la UI («Teclado y mouse»).
    pub name: String,
    /// Qué hace en esta máquina (la línea bajo el nombre).
    pub detail: String,
    pub state: ModuleState,
    /// El usuario lo puede encender y apagar ([`UiCommand::SetModule`]).
    pub switchable: bool,
}

/// Un monitor con canal DDC vivo desde la máquina que lo anuncia (T-110): contestó al
/// sondeo de solo lectura, sea compatible o no.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitorChannel {
    /// `<modelo>-<hash corto del serial del EDID>`: el mismo por DP y por HDMI.
    pub id: String,
    /// El modelo, para la UI.
    pub name: String,
    pub compat: MonitorCompat,
    /// Lo que tardó en contestar el sondeo, medido al abrir el canal (no cambia mientras
    /// el canal siga vivo, para que la elección del dueño no oscile).
    pub latency_ms: u32,
}

/// Si un monitor habla el estándar del firmware propio (DECISIONS D11).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MonitorCompat {
    Compatible {
        protocol: u8,
    },
    /// Se muestra, pero nunca se le escribe nada.
    NotCompatible {
        reason: String,
    },
}

impl MonitorCompat {
    pub fn is_compatible(&self) -> bool {
        matches!(self, MonitorCompat::Compatible { .. })
    }
}

/// Lo que leyó el dueño de un monitor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitorReport {
    pub id: String,
    /// La entrada que está en pantalla (la real, de `0xEE`); `None` si no se pudo leer.
    pub input: Option<MonitorInput>,
    pub power_on: Option<bool>,
    pub signal: Option<bool>,
    /// Los últimos eventos, del más viejo al más nuevo.
    pub events: Vec<MonitorEvent>,
}

/// Un evento del firmware, con la hora ya pasada al reloj del dueño.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitorEvent {
    /// Milisegundos desde la época Unix, según el reloj de la máquina dueña.
    pub at_ms: u64,
    pub kind: MonitorEventKind,
    /// La entrada de la que habla, si habla de una.
    pub input: Option<MonitorInput>,
}

/// Qué pasó, para el ícono y el texto de la UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MonitorEventKind {
    /// El monitor arrancó (o se reinició).
    Boot,
    /// Cambió la entrada en pantalla (por DDC o desde el menú del monitor).
    Input,
    /// Alguien pidió un cambio de entrada por DDC.
    SwitchRequested,
    /// El firmware confirmó el cambio: imagen estable en la entrada nueva.
    SwitchDone,
    /// El cambio no dio imagen; el monitor sigue en la de antes.
    SwitchFailed,
    /// Se encendió o se apagó.
    Power,
    /// Ganó o perdió la señal.
    Signal,
    /// Cambió un ajuste del menú.
    Setting,
    /// Se perdieron eventos (el anillo dio la vuelta entre dos lecturas).
    Lost,
    Other,
}

/// Un monitor como lo ve la UI (pantalla Monitores, mockup `V3-Monitores`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitorView {
    pub id: String,
    pub name: String,
    pub compat: MonitorCompat,
    /// Las máquinas con canal DDC vivo hacia él.
    pub seen_by: Vec<Machine>,
    /// Quién lo lee y lo controla; `None` si no es compatible o nadie tiene canal.
    pub owner: Option<Machine>,
    /// Lo último que leyó el dueño; `None` mientras no haya lectura.
    pub report: Option<MonitorReport>,
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
    /// Un pedido de túnel ([`crate::TunnelUp`]): lo abre el origen. Si es
    /// [`crate::TunnelUp::Attach`], el stream pasa a ser el canal de datos del túnel.
    Tunnel,
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
    /// Encender o apagar un módulo de `machine` (de esta o de la otra). Es un cambio del
    /// puesto compartido (`Desk.modules`, D4), así que llega a la otra con el puesto; el
    /// estado nuevo llega en [`AgentToUi::Machines`].
    SetModule {
        machine: Machine,
        module: ModuleId,
        on: bool,
    },
    /// Cambiar el puesto compartido (DESIGN §4.4): se aplica aquí, se guarda y va a la otra
    /// máquina. Contesta [`Reply::Desk`] con el puesto que queda, o el motivo del rechazo.
    EditDesk {
        patch: DeskPatch,
    },
    /// Volver a la versión `to_rev` del historial (D5, «Deshacer»): es un cambio nuevo con
    /// exactamente el puesto de aquella.
    UndoDesk {
        to_rev: u64,
    },
    /// Las últimas versiones del puesto que tuvo esta máquina.
    GetDeskHistory,
    /// Los monitores de las dos máquinas ([`MonitorView`]).
    GetMonitors,
    /// Cambiar la entrada de un monitor compatible: una transacción con el veredicto del
    /// firmware, que hace su dueño (se contesta [`Reply::Accepted`]).
    SwitchMonitor {
        monitor: String,
        input: MonitorInput,
    },
    /// Volver a escanear los entornos de `machine`; lo encontrado llega en
    /// [`AgentToUi::Machines`] (T-105).
    Rescan {
        machine: Machine,
    },
    /// Recibir (o dejar de recibir) [`AgentToUi::Resources`] de las dos máquinas por esta
    /// conexión. Se muestrea solo mientras alguien mira; al cerrarse la conexión se deja.
    WatchResources {
        on: bool,
    },
    /// Las versiones de las dos máquinas (T-122).
    GetUpdate,
    /// Pedirle a la otra máquina que instale `version` (T-122); su avance se ve en
    /// [`AgentToUi::Update`] cuando vuelve con la versión nueva.
    UpdatePeer {
        version: String,
    },
    /// Aplicar la escena `id` del puesto (D6): una transacción (se contesta
    /// [`Reply::Accepted`]). Crear, editar y borrar escenas son parches del puesto
    /// ([`UiCommand::EditDesk`]).
    ApplyScene {
        id: String,
    },
    /// Abrir un túnel hacia `target` (la otra máquina). Solo desde un
    /// [`Peer::Client`]; contesta [`Reply::Tunnel`] (T-103).
    OpenTunnel {
        title: String,
        project: Option<Project>,
        target: Machine,
        /// La sesión del cliente (la de Claude).
        session: String,
    },
    /// Cerrar un túnel abierto desde esta máquina: se cierran sus recursos en la destino.
    /// Solo desde un [`Peer::Client`].
    CloseTunnel {
        id: TunnelId,
    },
    /// Los túneles de las dos máquinas, agrupados por proyecto ([`Reply::Tunnels`]).
    GetTunnels,
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
    /// Aviso espontáneo cuando cambia lo que anuncia alguna de las dos máquinas (sistema,
    /// nombre, color, módulos, entornos, monitores). `remote` es `None` si la otra nunca se
    /// vio.
    Machines {
        local: Box<Capabilities>,
        remote: Option<Box<Capabilities>>,
    },
    /// Avance de la prueba «presiona una tecla» del asistente (T-087).
    KeyProbe(KeyProbeStatus),
    /// La otra máquina cambió el puesto y ganó (D5): el aviso «Cambiado desde X ·
    /// Deshacer». Deshacer es [`UiCommand::UndoDesk`] con `undo_to`.
    DeskChanged {
        by: Machine,
        rev: u64,
        /// Qué secciones cambiaron («Acomodo y atajos»).
        summary: String,
        /// La versión que había aquí antes del cambio.
        undo_to: u64,
    },
    /// Aviso espontáneo cuando cambia algún monitor (canal, dueño, entrada, eventos).
    Monitors(Vec<MonitorView>),
    /// Recursos en vivo de `machine`, a las conexiones que pidieron
    /// [`UiCommand::WatchResources`] (T-105).
    Resources {
        machine: Machine,
        resources: Box<Resources>,
    },
    /// Las versiones de las dos máquinas y si la otra pidió actualizar esta (T-122), con
    /// cada cambio.
    Update(UpdateStatus),
    /// Aviso espontáneo cuando cambia algún túnel abierto (T-103): agrupados por proyecto,
    /// solo para mirar (D9).
    Tunnels(Vec<ProjectTunnels>),
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
    /// Latencia de la entrada (captura → inyección) de la última ventana de 5 s, medida
    /// por quien recibe la entrada (T-093). `None` hasta la primera medición.
    #[serde(default)]
    pub latency: Option<InputLatency>,
}

/// Latencia de la entrada en una ventana: de la marca de tiempo del servidor (pasada al
/// reloj del cliente con el desfase estimado por los latidos) a la inyección.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputLatency {
    /// El enlace por el que se midió; `None` si quien la mide no lo sabe.
    pub link: Option<Link>,
    /// Eventos medidos en la ventana.
    pub count: u32,
    pub p50_us: u32,
    pub p99_us: u32,
    pub max_us: u32,
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
            latency: None,
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
    /// El historial del puesto, de la versión más nueva a la más vieja.
    DeskHistory(Vec<HistoryEntry>),
    Monitors(Vec<MonitorView>),
    /// Las versiones de las dos máquinas (T-122).
    Update(UpdateStatus),
    /// El túnel que se abrió.
    Tunnel(Box<Tunnel>),
    Tunnels(Vec<ProjectTunnels>),
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
    /// Vuelta atrás: el paso `index`, ya hecho, se deshizo porque falló uno posterior.
    StepUndone {
        txn: u64,
        index: u32,
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

    use tandem_core::{Align, Arrangement, DeskStamp, Scale, ScreenSpec, Side};

    use super::*;

    pub fn state() -> State {
        State {
            on: tandem_core::Powered::both(),
            scene: Some("trabajo".into()),
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
            arrangement: Some(Arrangement {
                a_side: Side::Up,
                align: Align::End,
            }),
            scenes: tandem_core::default_scenes(
                &tandem_core::Machines {
                    a: Some(MachineInfo::new(Machine::A, tandem_core::Os::MacOs, None)),
                    b: Some(MachineInfo::new(
                        Machine::B,
                        tandem_core::Os::Windows,
                        Some("ESCRITORIO".into()),
                    )),
                },
                None,
            ),
            ..Config::default()
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
            Step::SwitchMonitor {
                monitor: "lc34g55t-0a1b2c3d".into(),
                input: MonitorInput::Hdmi,
            },
            Step::ShowMachine {
                monitor: "lc34g55t-0a1b2c3d".into(),
                machine: Machine::B,
            },
            Step::Arrange {
                layout: Some(Arrangement::default()),
            },
            Step::Geometry,
        ]
    }

    pub fn step_arm(s: &Step) -> usize {
        match s {
            Step::PowerOn { .. } => 0,
            Step::PowerOff { .. } => 1,
            Step::SwitchMonitor { .. } => 2,
            Step::ShowMachine { .. } => 3,
            Step::Arrange { .. } => 4,
            Step::Geometry => 5,
        }
    }

    pub fn monitor_report() -> MonitorReport {
        MonitorReport {
            id: "lc34g55t-0a1b2c3d".into(),
            input: Some(MonitorInput::DisplayPort),
            power_on: Some(true),
            signal: None,
            events: vec![
                MonitorEvent {
                    at_ms: 1_763_000_000_000,
                    kind: MonitorEventKind::SwitchDone,
                    input: Some(MonitorInput::Hdmi),
                },
                MonitorEvent {
                    at_ms: 0,
                    kind: MonitorEventKind::Lost,
                    input: None,
                },
            ],
        }
    }

    pub fn monitor_views() -> Vec<MonitorView> {
        vec![
            MonitorView {
                id: "lc34g55t-0a1b2c3d".into(),
                name: "LC34G55T".into(),
                compat: MonitorCompat::Compatible { protocol: 2 },
                seen_by: vec![Machine::A, Machine::B],
                owner: Some(Machine::B),
                report: Some(monitor_report()),
            },
            MonitorView {
                id: "odyssey-0f0f0f0f".into(),
                name: "Odyssey G5".into(),
                compat: MonitorCompat::NotCompatible {
                    reason: "firmware de fábrica".into(),
                },
                seen_by: vec![Machine::B],
                owner: None,
                report: None,
            },
        ]
    }

    pub fn monitor_event_kind_arm(k: &MonitorEventKind) -> usize {
        match k {
            MonitorEventKind::Boot => 0,
            MonitorEventKind::Input => 1,
            MonitorEventKind::SwitchRequested => 2,
            MonitorEventKind::SwitchDone => 3,
            MonitorEventKind::SwitchFailed => 4,
            MonitorEventKind::Power => 5,
            MonitorEventKind::Signal => 6,
            MonitorEventKind::Setting => 7,
            MonitorEventKind::Lost => 8,
            MonitorEventKind::Other => 9,
        }
    }

    pub fn monitor_event_kinds() -> Vec<MonitorEventKind> {
        use MonitorEventKind::*;
        vec![
            Boot,
            Input,
            SwitchRequested,
            SwitchDone,
            SwitchFailed,
            Power,
            Signal,
            Setting,
            Lost,
            Other,
        ]
    }

    pub fn hellos() -> Vec<Hello> {
        vec![
            Hello::new(Peer::Agent(Machine::A)),
            Hello::new(Peer::Agent(Machine::B)),
            Hello::new(Peer::Ui),
            Hello::new(Peer::Client(crate::ClientKind::Mcp)),
            Hello::new(Peer::Client(crate::ClientKind::Cli)),
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
            ControlMsg::Open(Channel::Tunnel),
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
            ControlMsg::WatchResources { on: true },
            ControlMsg::WatchResources { on: false },
            ControlMsg::Rescan,
            ControlMsg::Monitors(vec![]),
            ControlMsg::Monitors(vec![monitor_report()]),
            ControlMsg::Update(UpdateMsg::Version("0.3.0".into())),
            ControlMsg::Update(UpdateMsg::Install {
                version: "0.3.1-rc.2".into(),
            }),
            ControlMsg::Focus(FocusMsg::Claim { epoch: 0 }),
            ControlMsg::Focus(FocusMsg::Grant { epoch: u64::MAX }),
            ControlMsg::Focus(FocusMsg::State {
                epoch: 3,
                server: Machine::A,
            }),
            ControlMsg::Desk(Box::new(DeskVersion {
                stamp: DeskStamp {
                    rev: 4,
                    at: 1_763_400_000_000,
                    by: Machine::B,
                },
                desk: desk(),
            })),
            ControlMsg::Desk(Box::new(DeskVersion {
                stamp: DeskStamp::fresh(Machine::A),
                desk: Config::default(),
            })),
        ];
        v.extend(
            capabilities()
                .into_iter()
                .map(|c| ControlMsg::Capabilities(Box::new(c))),
        );
        v.extend(
            crate::env::samples::resources()
                .into_iter()
                .map(|r| ControlMsg::Resources(Box::new(r))),
        );
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
            ControlMsg::Monitors(_) => 13,
            ControlMsg::WatchResources { .. } => 14,
            ControlMsg::Resources(_) => 15,
            ControlMsg::Rescan => 16,
            ControlMsg::Update(_) => 17,
            ControlMsg::Focus(_) => 18,
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
                modules: modules(),
                environments: crate::env::samples::environments(),
                hardware: crate::env::samples::hardware(),
                monitors: vec![
                    MonitorChannel {
                        id: "lc34g55t-0a1b2c3d".into(),
                        name: "LC34G55T".into(),
                        compat: MonitorCompat::Compatible { protocol: 2 },
                        latency_ms: 42,
                    },
                    MonitorChannel {
                        id: "odyssey-0f0f0f0f".into(),
                        name: "Odyssey G5".into(),
                        compat: MonitorCompat::NotCompatible {
                            reason: "firmware de fábrica".into(),
                        },
                        latency_ms: 0,
                    },
                ],
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
                modules: vec![],
                environments: vec![],
                hardware: Hardware::default(),
                monitors: vec![],
            },
        ]
    }

    /// Un módulo en cada estado.
    pub fn modules() -> Vec<ModuleInfo> {
        vec![
            ModuleInfo {
                id: ModuleId::Input,
                name: "Teclado y mouse".into(),
                detail: "Captura e inyección".into(),
                state: ModuleState::Active,
                switchable: true,
            },
            ModuleInfo {
                id: ModuleId::Clipboard,
                name: "Portapapeles".into(),
                detail: "Texto, HTML e imagen".into(),
                state: ModuleState::Off,
                switchable: true,
            },
            ModuleInfo {
                id: ModuleId::Monitors,
                name: "Monitores".into(),
                detail: String::new(),
                state: ModuleState::Unavailable {
                    reason: "Sin canal DDC en esta máquina".into(),
                },
                switchable: false,
            },
        ]
    }

    pub fn module_state_arm(s: &ModuleState) -> usize {
        match s {
            ModuleState::Active => 0,
            ModuleState::Off => 1,
            ModuleState::Unavailable { .. } => 2,
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
                latency: Some(InputLatency {
                    link: Some(Link::Wifi),
                    count: 5000,
                    p50_us: 900,
                    p99_us: 3_200,
                    max_us: 12_000,
                }),
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
            UiCommand::SetModule {
                machine: Machine::B,
                module: ModuleId::SecureDesktop,
                on: true,
            },
            UiCommand::UndoDesk { to_rev: 3 },
            UiCommand::GetDeskHistory,
            UiCommand::GetMonitors,
            UiCommand::SwitchMonitor {
                monitor: "lc34g55t-0a1b2c3d".into(),
                input: MonitorInput::DisplayPort,
            },
            UiCommand::Rescan {
                machine: Machine::A,
            },
            UiCommand::WatchResources { on: true },
            UiCommand::WatchResources { on: false },
            UiCommand::GetUpdate,
            UiCommand::UpdatePeer {
                version: "0.3.1".into(),
            },
            UiCommand::ApplyScene {
                id: "trabajo".into(),
            },
            UiCommand::OpenTunnel {
                title: "Corre los tests".into(),
                project: tunnel_sample().project,
                target: Machine::B,
                session: "claude-1234".into(),
            },
            UiCommand::OpenTunnel {
                title: String::new(),
                project: None,
                target: Machine::A,
                session: String::new(),
            },
            UiCommand::CloseTunnel {
                id: crate::TunnelId(u64::MAX),
            },
            UiCommand::GetTunnels,
        ];
        let mut cmds = cmds;
        cmds.extend(
            desk_patches()
                .into_iter()
                .map(|patch| UiCommand::EditDesk { patch }),
        );
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
            UiCommand::SetModule { .. } => 17,
            UiCommand::EditDesk { .. } => 18,
            UiCommand::UndoDesk { .. } => 19,
            UiCommand::GetDeskHistory => 20,
            UiCommand::GetMonitors => 21,
            UiCommand::SwitchMonitor { .. } => 22,
            UiCommand::Rescan { .. } => 23,
            UiCommand::WatchResources { .. } => 24,
            UiCommand::UpdatePeer { .. } => 25,
            UiCommand::GetUpdate => 26,
            UiCommand::ApplyScene { .. } => 27,
            UiCommand::OpenTunnel { .. } => 28,
            UiCommand::CloseTunnel { .. } => 29,
            UiCommand::GetTunnels => 30,
        }
    }

    /// Un parche de cada sección del puesto.
    pub fn desk_patches() -> Vec<DeskPatch> {
        vec![
            DeskPatch::Machine {
                side: Machine::B,
                name: Some("Torre".into()),
                color: Some("#5fbfb0".into()),
            },
            DeskPatch::Machine {
                side: Machine::A,
                name: None,
                color: None,
            },
            DeskPatch::Arrangement(None),
            DeskPatch::Arrangement(Some(Arrangement {
                a_side: Side::Down,
                align: Align::Start,
            })),
            DeskPatch::Screens {
                machine: Machine::A,
                screens: desk().screens,
            },
            DeskPatch::Scenes(desk().scenes),
            DeskPatch::AddScene(tandem_core::default_scenes(&desk().machines, None).remove(2)),
            DeskPatch::EditScene(tandem_core::Scene {
                monitors: vec![("lc34g55t-0a1b2c3d".into(), Machine::A)],
                require_on: Some(Machine::A),
                ..tandem_core::Scene::new("trabajo", "Trabajo")
            }),
            DeskPatch::DeleteScene { id: "x".into() },
            DeskPatch::Shortcuts(tandem_core::Shortcuts { lock: vec![] }),
            DeskPatch::Prefs(tandem_core::InputPrefs {
                cmd_ctrl: false,
                drag_files: true,
            }),
            DeskPatch::Module {
                machine: Machine::B,
                module: "clipboard".into(),
                on: false,
            },
        ]
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
            TxnEvent::StepUndone { txn: 1, index: 0 },
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
            TxnEvent::StepUndone { .. } => 6,
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
        v.push(AgentToUi::DeskChanged {
            by: Machine::B,
            rev: 7,
            summary: "Acomodo y atajos".into(),
            undo_to: 6,
        });
        v.push(AgentToUi::Reply {
            id: 14,
            reply: Reply::DeskHistory(vec![
                HistoryEntry {
                    stamp: DeskStamp {
                        rev: 7,
                        at: 9,
                        by: Machine::B,
                    },
                    summary: "Acomodo".into(),
                },
                HistoryEntry {
                    stamp: DeskStamp::fresh(Machine::A),
                    summary: String::new(),
                },
            ]),
        });
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
        v.push(AgentToUi::Machines {
            local: Box::new(mac.clone()),
            remote: Some(Box::new(pc)),
        });
        v.push(AgentToUi::Machines {
            local: Box::new(mac.clone()),
            remote: None,
        });
        v.push(AgentToUi::Reply {
            id: 11,
            reply: Reply::Capabilities(Box::new(DeskCapabilities {
                here: mac,
                peer: None,
            })),
        });
        v.push(AgentToUi::Monitors(vec![]));
        v.push(AgentToUi::Tunnels(vec![]));
        v.push(AgentToUi::Tunnels(crate::group_by_project([
            tunnel_sample(),
        ])));
        v.push(AgentToUi::Reply {
            id: 13,
            reply: Reply::Tunnel(Box::new(tunnel_sample())),
        });
        v.push(AgentToUi::Reply {
            id: 14,
            reply: Reply::Tunnels(crate::group_by_project([tunnel_sample()])),
        });
        v.push(AgentToUi::Monitors(monitor_views()));
        v.push(AgentToUi::Reply {
            id: 12,
            reply: Reply::Monitors(monitor_views()),
        });
        v.extend(
            crate::env::samples::resources()
                .into_iter()
                .map(|r| AgentToUi::Resources {
                    machine: Machine::B,
                    resources: Box::new(r),
                }),
        );
        v.extend(links().into_iter().map(|link| {
            AgentToUi::Peer(PeerStatus::Paired {
                name: "Mac".into(),
                link: Some(link),
            })
        }));
        v.push(AgentToUi::Update(UpdateStatus::default()));
        v.push(AgentToUi::Reply {
            id: 14,
            reply: Reply::Update(UpdateStatus {
                here: "0.3.0".into(),
                peer: None,
                install: None,
                peer_protocol: None,
            }),
        });
        v.push(AgentToUi::Update(UpdateStatus {
            here: "0.3.0".into(),
            peer: Some("0.3.1".into()),
            install: Some("0.3.1".into()),
            peer_protocol: Some(PROTOCOL_VERSION + 1),
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
            AgentToUi::Machines { .. } => 8,
            AgentToUi::KeyProbe(_) => 9,
            AgentToUi::DeskChanged { .. } => 10,
            AgentToUi::Monitors(_) => 11,
            AgentToUi::Resources { .. } => 12,
            AgentToUi::Update(_) => 13,
            AgentToUi::Tunnels(_) => 14,
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
            Reply::DeskHistory(_) => 11,
            Reply::Monitors(_) => 12,
            Reply::Update(_) => 13,
            Reply::Tunnel(_) => 14,
            Reply::Tunnels(_) => 15,
        }
    }

    pub fn peer_arm(p: &Peer) -> usize {
        match p {
            Peer::Agent(Machine::A) => 0,
            Peer::Agent(Machine::B) => 1,
            Peer::Ui => 2,
            Peer::Client(_) => 3,
        }
    }

    pub fn tunnel_sample() -> crate::Tunnel {
        crate::Tunnel {
            id: crate::TunnelId(0x1234_5678_9abc_def0),
            title: "Corre los tests en la Mac".into(),
            project: Some(crate::Project {
                name: "tandem".into(),
                root: "/home/u/projects/tandem".into(),
            }),
            origin: Machine::B,
            target: Machine::A,
            opened_by: crate::Client {
                kind: crate::ClientKind::Mcp,
                session: "claude-1234".into(),
            },
            state: crate::TunnelState::Open,
            created: 1_763_400_000_000,
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
        covers(&steps(), step_arm, 6);
        covers(&hellos(), |h| peer_arm(&h.peer), 4);
        covers(&control(), control_arm, 19);
        let cmds: Vec<_> = ui_requests().into_iter().map(|r| r.command).collect();
        covers(&cmds, ui_command_arm, 31);
        covers(&modules(), |m| module_state_arm(&m.state), 3);
        covers(&to_ui(), to_ui_arm, 15);
        covers(&monitor_event_kinds(), monitor_event_kind_arm, 10);
        let kinds: Vec<_> = capabilities()
            .iter()
            .flat_map(|c| c.environments.iter().map(|e| e.kind))
            .collect();
        covers(&kinds, |k| crate::env::samples::env_kind_arm(*k), 3);
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
        covers(&replies, reply_arm, 16);
        covers(&txn_events(), txn_arm, 7);
    }

    #[test]
    fn module_ids_have_a_stable_text() {
        for m in ModuleId::ALL {
            assert_eq!(ModuleId::parse(m.as_str()), Some(m));
        }
        assert_eq!(ModuleId::Clipboard.as_str(), "clipboard");
        assert_eq!(ModuleId::parse("Clipboard"), None);
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
    fn every_monitor_event_kind_round_trips() {
        round_trip(&monitor_event_kinds());
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
