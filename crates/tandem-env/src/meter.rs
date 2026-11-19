//! CPU, RAM, disk and temperatures, behind [`Meter`]. On the real machine
//! [`SysinfoMeter`] is used; tests use [`crate::fake::FakeMeter`].

use tandem_proto::{CpuLoad, DiskSpace, GpuLoad, MemoryUse, Resources};

use crate::Platform;
use crate::parse::tenths;

/// The fixed facts of the machine.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MeterInfo {
    pub cpu: Option<String>,
    /// Logical cores.
    pub cores: u32,
    /// Total RAM, in bytes.
    pub memory: u64,
}

/// A mounted volume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskReading {
    pub mount: String,
    pub free: u64,
    pub total: u64,
}

/// A reading of the system counters.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Reading {
    /// Total usage, 0–100.
    pub cpu: f32,
    /// Usage of each logical core, 0–100.
    pub cores: Vec<f32>,
    /// Temperature sensors (label, °C).
    pub temperatures: Vec<(String, f32)>,
    pub memory_used: u64,
    pub memory_total: u64,
    pub disks: Vec<DiskReading>,
}

/// The system counters.
pub trait Meter: Send {
    fn info(&mut self) -> MeterInfo;
    /// CPU usage is since the previous reading.
    fn read(&mut self) -> Reading;
}

/// The CPU temperature: the hottest of the sensors that belong to the CPU
/// (`Tctl`/`Tdie` on AMD, `Package` on Intel, `PMU tdie` on Apple Silicon). Impossible
/// values are discarded.
pub fn cpu_temperature(sensors: &[(String, f32)]) -> Option<i16> {
    const CPU: [&str; 6] = ["tctl", "tdie", "package", "cpu", "core", "soc"];
    sensors
        .iter()
        .filter(|(label, _)| {
            let l = label.to_ascii_lowercase();
            CPU.iter().any(|k| l.contains(k)) && !l.contains("gpu")
        })
        .map(|(_, t)| *t)
        .filter(|t| t.is_finite() && *t > 0.0 && *t < 150.0)
        .fold(None, |max: Option<f32>, t| {
            Some(max.map_or(t, |m| m.max(t)))
        })
        .map(|t| t.round() as i16)
}

/// The system volume: `C:\` (or the `SystemDrive` one) on Windows, the data one on macOS
/// (on APFS the system one is read-only), `/` on Linux. If it does not appear, the largest.
pub fn system_disk(
    disks: &[DiskReading],
    platform: Platform,
    system_drive: Option<&str>,
) -> Option<DiskSpace> {
    let wanted: Vec<String> = match platform {
        Platform::Windows => {
            let drive = system_drive.unwrap_or("C:").trim_end_matches('\\');
            vec![format!("{drive}\\")]
        }
        Platform::MacOs => vec!["/System/Volumes/Data".into(), "/".into()],
        Platform::Linux => vec!["/".into()],
    };
    wanted
        .iter()
        .find_map(|w| disks.iter().find(|d| d.mount.eq_ignore_ascii_case(w)))
        .or_else(|| disks.iter().max_by_key(|d| d.total))
        .map(|d| DiskSpace {
            free: d.free.min(d.total),
            total: d.total,
        })
}

/// The sample that travels, from a reading and the GPUs.
pub fn resources(
    reading: &Reading,
    gpus: Vec<GpuLoad>,
    platform: Platform,
    system_drive: Option<&str>,
) -> Resources {
    Resources {
        cpu: CpuLoad {
            load: tenths(reading.cpu),
            cores: reading.cores.iter().map(|c| tenths(*c)).collect(),
            temperature: cpu_temperature(&reading.temperatures),
        },
        memory: MemoryUse {
            used: reading.memory_used.min(reading.memory_total),
            total: reading.memory_total,
        },
        gpus,
        disk: system_disk(&reading.disks, platform, system_drive),
    }
}

/// The real counters, with `sysinfo`.
pub struct SysinfoMeter {
    system: Option<sysinfo::System>,
    disks: Option<sysinfo::Disks>,
    components: Option<sysinfo::Components>,
    primed: bool,
}

impl std::fmt::Debug for SysinfoMeter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SysinfoMeter")
            .field("primed", &self.primed)
            .finish_non_exhaustive()
    }
}

impl Default for SysinfoMeter {
    fn default() -> Self {
        Self::new()
    }
}

impl SysinfoMeter {
    pub fn new() -> Self {
        let system = sysinfo::System::new()
            .map_err(|e| tracing::warn!("cannot read the CPU or RAM: {e:?}"))
            .ok();
        SysinfoMeter {
            system,
            disks: sysinfo::Disks::new_with_refreshed_list().ok(),
            components: sysinfo::Components::new_with_refreshed_list().ok(),
            primed: false,
        }
    }
}

impl Meter for SysinfoMeter {
    fn info(&mut self) -> MeterInfo {
        let Some(sys) = self.system.as_mut() else {
            return MeterInfo::default();
        };
        sys.refresh_cpu_all();
        sys.refresh_memory();
        let cpu = sys
            .cpus()
            .first()
            .map(|c| c.brand().trim().to_owned())
            .filter(|b| !b.is_empty());
        MeterInfo {
            cpu,
            cores: u32::try_from(sys.cpus().len()).unwrap_or(u32::MAX),
            memory: sys.total_memory(),
        }
    }

    fn read(&mut self) -> Reading {
        let Some(sys) = self.system.as_mut() else {
            return Reading::default();
        };
        if !self.primed {
            // CPU usage comes from two readings separated by at least this interval.
            sys.refresh_cpu_usage();
            std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
            self.primed = true;
        }
        sys.refresh_cpu_usage();
        sys.refresh_memory();
        let disks = match self.disks.as_mut() {
            Some(d) => {
                d.refresh(true);
                d.iter()
                    .map(|d| DiskReading {
                        mount: d.mount_point().to_string_lossy().into_owned(),
                        free: d.available_space(),
                        total: d.total_space(),
                    })
                    .collect()
            }
            None => Vec::new(),
        };
        let temperatures = match self.components.as_mut() {
            Some(c) => {
                c.refresh(false);
                c.iter()
                    .filter_map(|c| Some((c.label().to_owned(), c.temperature()?)))
                    .collect()
            }
            None => Vec::new(),
        };
        Reading {
            cpu: sys.global_cpu_usage(),
            cores: sys.cpus().iter().map(sysinfo::Cpu::usage).collect(),
            temperatures,
            memory_used: sys.used_memory(),
            memory_total: sys.total_memory(),
            disks,
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn disk(mount: &str, free: u64, total: u64) -> DiskReading {
        DiskReading {
            mount: mount.into(),
            free,
            total,
        }
    }

    #[test]
    fn the_system_volume_is_picked_on_each_platform() {
        let win = [disk("D:\\", 5, 4000), disk("c:\\", 412, 1800)];
        assert_eq!(
            system_disk(&win, Platform::Windows, Some("C:")),
            Some(DiskSpace {
                free: 412,
                total: 1800
            })
        );
        let mac = [
            disk("/", 200, 994),
            disk("/System/Volumes/Data", 231, 994),
            disk("/Volumes/USB", 1, 64),
        ];
        assert_eq!(
            system_disk(&mac, Platform::MacOs, None).map(|d| d.free),
            Some(231)
        );
        let odd = [disk("/boot", 1, 2), disk("/data", 7, 9)];
        assert_eq!(
            system_disk(&odd, Platform::Linux, None).map(|d| d.free),
            Some(7)
        );
        assert_eq!(system_disk(&[], Platform::Linux, None), None);
    }

    #[test]
    fn the_cpu_temperature_is_the_hottest_cpu_sensor() {
        let sensors = [
            ("Tctl".to_owned(), 61.6),
            ("Composite NVMe".to_owned(), 70.0),
            ("Tccd1".to_owned(), 55.0),
            ("amdgpu edge".to_owned(), 80.0),
        ];
        assert_eq!(cpu_temperature(&sensors), Some(62));
        let apple = [
            ("PMU tdie1".to_owned(), 44.2),
            ("PMU tdie3".to_owned(), 48.0),
            ("gas gauge battery".to_owned(), 31.0),
        ];
        assert_eq!(cpu_temperature(&apple), Some(48));
        assert_eq!(cpu_temperature(&[("cpu".into(), f32::NAN)]), None);
        assert_eq!(cpu_temperature(&[]), None);
    }

    proptest! {
        #[test]
        fn a_sample_is_always_in_range(
            cpu in proptest::num::f32::ANY,
            cores in proptest::collection::vec(proptest::num::f32::ANY, 0..32),
            used in any::<u64>(),
            total in any::<u64>(),
            free in any::<u64>(),
            dtotal in any::<u64>(),
        ) {
            let r = Reading {
                cpu,
                cores,
                temperatures: vec![],
                memory_used: used,
                memory_total: total,
                disks: vec![disk("/", free, dtotal)],
            };
            let s = resources(&r, vec![], Platform::Linux, None);
            prop_assert!(s.cpu.load <= 1000);
            prop_assert!(s.cpu.cores.iter().all(|c| *c <= 1000));
            prop_assert!(s.memory.used <= s.memory.total);
            let d = s.disk.unwrap();
            prop_assert!(d.free <= d.total);
        }
    }
}
