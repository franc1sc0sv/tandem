//! Topología real por la API CCD (`QueryDisplayConfig`/`SetDisplayConfig`).
//!
//! Se consulta siempre con `QDC_ALL_PATHS` (rutas activas e inactivas) y se decide con el
//! planificador portable. Sin `QDC_VIRTUAL_MODE_AWARE`, así los índices de modo de cada ruta
//! son índices planos y no campos de bits.

use std::collections::BTreeMap;

use ::windows::Win32::Devices::Display::{
    DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO,
    DISPLAYCONFIG_TARGET_DEVICE_NAME, DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes,
    QDC_ALL_PATHS, QueryDisplayConfig, SDC_ALLOW_CHANGES, SDC_APPLY, SDC_SAVE_TO_DATABASE,
    SDC_USE_SUPPLIED_DISPLAY_CONFIG, SET_DISPLAY_CONFIG_FLAGS, SetDisplayConfig,
};
use ::windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS, LUID};
use ::windows::core::HRESULT;

use super::ddc::{read_edid, wide_to_string};
use crate::ddc::edid;
use crate::topology::plan::{self, MODE_IDX_INVALID, PathSlot};
use crate::topology::{Display, DisplayTopology, TargetId, TopologyError};

/// `DISPLAYCONFIG_PATH_ACTIVE` (wingdi.h); el crate `windows` no lo exporta.
const PATH_ACTIVE: u32 = 0x1;
/// La topología puede cambiar entre pedir el tamaño y consultar; se reintenta unas veces.
const QUERY_ATTEMPTS: usize = 4;
/// Se guarda en la base de datos de Windows para que un replug o un cambio de entrada del
/// monitor no deshaga el cambio. `SDC_ALLOW_CHANGES` deja que Windows elija el modo de un
/// monitor recién activado.
const SET_FLAGS: SET_DISPLAY_CONFIG_FLAGS = SET_DISPLAY_CONFIG_FLAGS(
    SDC_APPLY.0 | SDC_USE_SUPPLIED_DISPLAY_CONFIG.0 | SDC_ALLOW_CHANGES.0 | SDC_SAVE_TO_DATABASE.0,
);

fn os_error(op: &'static str, code: u32) -> TopologyError {
    TopologyError::Os {
        op,
        code,
        message: HRESULT::from_win32(code).message(),
    }
}

fn luid_to_u64(luid: LUID) -> u64 {
    (u64::from(luid.HighPart as u32) << 32) | u64::from(luid.LowPart)
}

/// Topología del escritorio de Windows. No guarda estado: cada operación consulta de nuevo.
#[derive(Debug, Default)]
pub struct WindowsTopology;

struct Config {
    paths: Vec<DISPLAYCONFIG_PATH_INFO>,
    modes: Vec<DISPLAYCONFIG_MODE_INFO>,
}

impl Config {
    fn query() -> Result<Self, TopologyError> {
        for _ in 0..QUERY_ATTEMPTS {
            let (mut np, mut nm) = (0u32, 0u32);
            // SAFETY: los dos punteros apuntan a u32 locales.
            let status = unsafe { GetDisplayConfigBufferSizes(QDC_ALL_PATHS, &mut np, &mut nm) };
            if status != ERROR_SUCCESS {
                return Err(os_error("GetDisplayConfigBufferSizes", status.0));
            }
            let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); np as usize];
            let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); nm as usize];
            // SAFETY: los arreglos tienen exactamente `np` y `nm` elementos, como dicen los
            // contadores; con QDC_ALL_PATHS el id de topología debe ser nulo.
            let status = unsafe {
                QueryDisplayConfig(
                    QDC_ALL_PATHS,
                    &mut np,
                    paths.as_mut_ptr(),
                    &mut nm,
                    modes.as_mut_ptr(),
                    None,
                )
            };
            if status == ERROR_INSUFFICIENT_BUFFER {
                continue;
            }
            if status != ERROR_SUCCESS {
                return Err(os_error("QueryDisplayConfig", status.0));
            }
            paths.truncate(np as usize);
            modes.truncate(nm as usize);
            return Ok(Self { paths, modes });
        }
        Err(os_error("QueryDisplayConfig", ERROR_INSUFFICIENT_BUFFER.0))
    }

    fn slots(&self) -> Vec<PathSlot> {
        self.paths
            .iter()
            .map(|p| PathSlot {
                // La fuente y el target de una ruta están en el mismo adaptador salvo en
                // equipos híbridos; para decidir basta el del target.
                adapter: luid_to_u64(p.targetInfo.adapterId),
                source: p.sourceInfo.id,
                target: p.targetInfo.id,
                active: p.flags & PATH_ACTIVE != 0,
                available: p.targetInfo.targetAvailable.as_bool(),
            })
            .collect()
    }

    fn luid(&self, target: TargetId) -> Option<LUID> {
        self.paths
            .iter()
            .map(|p| p.targetInfo.adapterId)
            .find(|l| luid_to_u64(*l) == target.adapter)
    }

    /// Aplica: deja activas las rutas activas menos `off`, más `on`.
    fn apply(
        &self,
        slots: &[PathSlot],
        on: Option<usize>,
        off: &[usize],
    ) -> Result<(), TopologyError> {
        let mut paths: Vec<DISPLAYCONFIG_PATH_INFO> = Vec::new();
        for (i, p) in self.paths.iter().enumerate() {
            if Some(i) == on {
                let mut p = *p;
                p.flags |= PATH_ACTIVE;
                // Sin modo: Windows elige resolución y posición (SDC_ALLOW_CHANGES).
                p.sourceInfo.Anonymous.modeInfoIdx = MODE_IDX_INVALID;
                p.targetInfo.Anonymous.modeInfoIdx = MODE_IDX_INVALID;
                paths.push(p);
            } else if slots[i].active && !off.contains(&i) {
                paths.push(*p);
            }
        }
        let mut refs: Vec<u32> = paths
            .iter()
            .flat_map(|p| {
                // SAFETY: sin QDC_VIRTUAL_MODE_AWARE la unión siempre contiene el índice plano.
                unsafe {
                    [
                        p.sourceInfo.Anonymous.modeInfoIdx,
                        p.targetInfo.Anonymous.modeInfoIdx,
                    ]
                }
            })
            .collect();
        let keep = plan::compact_modes(&mut refs, self.modes.len());
        for (p, [source, target]) in paths.iter_mut().zip(refs.as_chunks::<2>().0) {
            p.sourceInfo.Anonymous.modeInfoIdx = *source;
            p.targetInfo.Anonymous.modeInfoIdx = *target;
        }
        let modes: Vec<DISPLAYCONFIG_MODE_INFO> = keep.iter().map(|&k| self.modes[k]).collect();
        // SAFETY: los slices viven durante la llamada y los índices de modo apuntan dentro
        // de `modes` (o son inválidos).
        let status = unsafe { SetDisplayConfig(Some(&paths), Some(&modes), SET_FLAGS) };
        if status != ERROR_SUCCESS.0 as i32 {
            return Err(os_error("SetDisplayConfig", status as u32));
        }
        Ok(())
    }
}

/// Nombre amigable y ruta de interfaz del monitor conectado en un target.
fn target_name(luid: LUID, target: u32) -> Result<(String, String), TopologyError> {
    let mut req = DISPLAYCONFIG_TARGET_DEVICE_NAME::default();
    req.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME;
    req.header.size = std::mem::size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32;
    req.header.adapterId = luid;
    req.header.id = target;
    // SAFETY: `req` empieza con el encabezado y `size` declara el tamaño de la estructura.
    let status = unsafe { DisplayConfigGetDeviceInfo(&mut req.header) };
    if status != ERROR_SUCCESS.0 as i32 {
        return Err(os_error("DisplayConfigGetDeviceInfo", status as u32));
    }
    Ok((
        wide_to_string(&req.monitorFriendlyDeviceName),
        wide_to_string(&req.monitorDevicePath),
    ))
}

impl DisplayTopology for WindowsTopology {
    fn displays(&mut self) -> Result<Vec<Display>, TopologyError> {
        let cfg = Config::query()?;
        let luids: BTreeMap<u64, LUID> = cfg
            .paths
            .iter()
            .map(|p| (luid_to_u64(p.targetInfo.adapterId), p.targetInfo.adapterId))
            .collect();
        let mut out = Vec::new();
        for (target, active) in plan::targets(&cfg.slots()) {
            let Some(luid) = luids.get(&target.adapter) else {
                continue;
            };
            // Un target sin nombre (por ejemplo, recién desconectado) se lista igual.
            let (name, device_path) = target_name(*luid, target.target).unwrap_or_default();
            let edid = edid::registry_key_from_interface(&device_path)
                .and_then(|key| read_edid(&key))
                .and_then(|bytes| edid::parse(&bytes).ok());
            out.push(Display {
                target,
                name,
                device_path,
                edid,
                active,
            });
        }
        Ok(out)
    }

    fn set_active(&mut self, target: TargetId, active: bool) -> Result<(), TopologyError> {
        let cfg = Config::query()?;
        if cfg.luid(target).is_none() {
            return Err(TopologyError::NotConnected(target));
        }
        let slots = cfg.slots();
        if active {
            match plan::plan_attach(&slots, target)? {
                Some(i) => cfg.apply(&slots, Some(i), &[]),
                None => Ok(()),
            }
        } else {
            let off = plan::plan_detach(&slots, target)?;
            if off.is_empty() {
                return Ok(());
            }
            cfg.apply(&slots, None, &off)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::ddc::{DdcBus, DdcError, VCP_INPUT_SOURCE, VcpValue};
    use crate::topology::{self, Detached};
    use crate::windows::ddc::WindowsDdc;

    /// Monitor de las pruebas en hardware. Por defecto el 34"; se cambia con la variable.
    fn monitor_query() -> String {
        std::env::var("TANDEM_TOPOLOGY_MONITOR").unwrap_or_else(|_| "LC34G55T".into())
    }

    /// Windows tarda en reconfigurar el escritorio y en volver a enumerar los monitores.
    fn settle() {
        std::thread::sleep(Duration::from_secs(3));
    }

    #[test]
    fn luid_packs_high_and_low_parts() {
        let l = LUID {
            LowPart: 0xDEAD_BEEF,
            HighPart: -1,
        };
        assert_eq!(luid_to_u64(l), 0xFFFF_FFFF_DEAD_BEEF);
        assert_eq!(luid_to_u64(LUID::default()), 0);
    }

    #[test]
    fn set_flags_include_supplied_config_and_database() {
        assert!(SET_FLAGS.contains(SDC_APPLY));
        assert!(SET_FLAGS.contains(SDC_USE_SUPPLIED_DISPLAY_CONFIG));
        assert!(SET_FLAGS.contains(SDC_SAVE_TO_DATABASE));
    }

    /// En el runner de CI puede no haber monitores (o solo uno virtual): consultar no debe
    /// colgarse ni entrar en pánico, y si responde, cada monitor aparece una sola vez.
    #[test]
    fn listing_on_any_machine_is_consistent() {
        if let Ok(ds) = WindowsTopology.displays() {
            let mut ids: Vec<_> = ds.iter().map(|d| d.target).collect();
            ids.dedup();
            assert_eq!(ids.len(), ds.len());
        }
    }

    #[test]
    fn unknown_target_is_not_connected() {
        let t = TargetId {
            adapter: 0xFFFF_FFFF_FFFF_FFFF,
            target: 0xFFFF,
        };
        match WindowsTopology.set_active(t, true) {
            Err(TopologyError::NotConnected(_)) | Err(TopologyError::Os { .. }) => {}
            other => panic!("se esperaba NotConnected: {other:?}"),
        }
    }

    /// Solo lee.
    #[test]
    #[ignore = "hardware"]
    fn lists_real_displays_with_edid() {
        let ds = WindowsTopology.displays().unwrap();
        for d in &ds {
            eprintln!("{d}");
        }
        assert!(ds.iter().any(|d| d.active));
        let m = topology::find(&ds, &monitor_query()).unwrap();
        assert!(m.edid.is_some(), "{m}: sin EDID");
    }

    /// Quita el monitor del escritorio y lo devuelve (también si la prueba falla a la mitad).
    #[test]
    #[ignore = "hardware"]
    fn detach_and_attach_round_trip() {
        let query = monitor_query();
        let mut topo = WindowsTopology;
        let before = topo.displays().unwrap();
        assert!(
            before.iter().filter(|d| d.active).count() >= 2,
            "hacen falta dos monitores activos para no dejar la PC sin pantalla"
        );
        assert!(topology::find(&before, &query).unwrap().active);
        {
            let guard = Detached::new(&mut topo, &query).unwrap();
            settle();
            let target = guard.target();
            let now = WindowsTopology.displays().unwrap();
            let d = now.iter().find(|d| d.target == target).unwrap();
            assert!(!d.active, "{d} sigue activo");
            guard.restore().unwrap();
        }
        settle();
        let after = topo.displays().unwrap();
        assert!(topology::find(&after, &query).unwrap().active);
    }

    fn read_input(bus: &mut WindowsDdc, monitor: usize) -> Result<VcpValue, DdcError> {
        // Los Samsung fallan lecturas sueltas al azar: se dan unos intentos.
        let mut last = Err(DdcError::NoSuchMonitor(monitor));
        for _ in 0..3 {
            last = bus.get_vcp(monitor, VCP_INPUT_SOURCE);
            if last.is_ok() {
                break;
            }
        }
        last
    }

    /// Spike S3: ¿sigue vivo el canal DDC con el monitor fuera del escritorio? Solo lee el
    /// VCP 0x60; no cambia la entrada. Imprime el resultado para la nota de investigación.
    #[test]
    #[ignore = "hardware"]
    fn s3_ddc_while_detached() {
        let query = monitor_query();
        let mut bus = WindowsDdc::open().unwrap();
        let idx = bus
            .monitors()
            .iter()
            .position(|m| m.matches(&query))
            .unwrap();
        eprintln!("S3 antes: {:?}", read_input(&mut bus, idx));

        let mut topo = WindowsTopology;
        let guard = Detached::new(&mut topo, &query).unwrap();
        settle();
        let old_handle = read_input(&mut bus, idx);
        let fresh = WindowsDdc::open().unwrap();
        let enumerated = fresh.monitors().iter().any(|m| m.matches(&query));
        eprintln!(
            "S3 desactivado: handle previo = {old_handle:?}; enumeración nueva lo ve = {enumerated}"
        );
        drop(fresh);
        guard.restore().unwrap();
        settle();

        let mut bus = WindowsDdc::open().unwrap();
        let idx = bus
            .monitors()
            .iter()
            .position(|m| m.matches(&query))
            .unwrap();
        eprintln!("S3 después: {:?}", read_input(&mut bus, idx));
    }
}
