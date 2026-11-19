//! Tandem core, platform independent.
//!
//! - [`model`]: the two sides (`A`/`B`, by public key), which machine is each and the
//!   shared desk ([`Desk`]).
//! - [`desk`]: how it is edited (patches), synchronized (last one wins) and undone the
//!   desk.
//! - [`legacy`]: Spanish identifiers saved before T-209, migrated when reading.
//! - [`data`]: the cap on data over Wi-Fi (`[data]` of the desk).
//! - [`state`]: the observed state (which machines are on).
//! - [`plan`]: powering on and off as an ordered list of steps.
//! - [`txn`]: transactional execution of a plan, with rollback.
//! - [`monitor`]: the inputs of a compatible monitor.
//! - [`scene`]: scenes (D6) and how they are applied.

pub mod data;
pub mod desk;
pub mod legacy;
pub mod model;
pub mod monitor;
pub mod plan;
pub mod scene;
pub mod state;
pub mod txn;

pub use data::{DEFAULT_WIFI_CAP_PERCENT, DataPrefs, DataPrefsError};
pub use desk::{
    DeskHistory, DeskPatch, DeskStamp, DeskVersion, HISTORY_LEN, HistoryEntry, InputPrefs, Modules,
    PatchError, Shortcuts, UndoError, valid_color,
};
pub use model::{
    Align, Arrangement, Config, DEFAULT_COLORS, Desk, Machine, MachineInfo, Machines, Os, PALETTE,
    Scale, ScreenSpec, Side, default_color,
};
pub use monitor::{CableEnd, Inputs, MonitorCable, MonitorInput};
pub use plan::{Abilities, Plan, PlanError, Step};
pub use scene::{MonitorId, Scene, default_scenes};
pub use state::{Powered, State};
pub use txn::{Executor, TxnError, TxnReport, run};
