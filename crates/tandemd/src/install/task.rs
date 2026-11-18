//! Arranque con los privilegios más altos (A7, T-104): una tarea programada de Windows que
//! lanza el agente al iniciar sesión con «los privilegios más altos» (`HighestAvailable`),
//! sin pedir confirmación en cada inicio. Los procesos de los túneles heredan esos
//! privilegios (D8).
//!
//! Registrar una tarea elevada pide administrador. Si el instalador no lo tiene, queda el
//! arranque normal de la clave `Run` y el agente corre sin elevar ([`AgentInstall::RunKey`]).
//!
//! Solo se registra si el ejecutable y su carpeta los escriben únicamente administradores
//! ([`PathAcl`]); si no, cualquier proceso del usuario cambiaría el `.exe` y lo correría
//! elevado sin UAC. Se registra por la API COM del Programador de tareas, con el XML en
//! memoria y sin archivos intermedios.
//!
//! La tarea se define con un XML y no con las opciones de `schtasks /Create`, porque esas
//! dejan los valores por omisión del Programador de tareas: detener el agente a los tres días
//! y no arrancarlo con batería, que no sirven para un agente que tiene que estar siempre.

use std::io;
use std::path::Path;

use super::{Autostart, InstallError, InstallReport, command_line};
use crate::cli::RunArgs;

/// Nombre de la tarea, en su carpeta propia del Programador de tareas.
pub const TASK_NAME: &str = r"Tandem\tandemd";

/// El Programador de tareas. En Windows lo hace su API COM ([`super::HighestTask`]).
pub trait Scheduler {
    /// El proceso actual tiene el token elevado (administrador con UAC aceptado).
    fn is_elevated(&self) -> bool;
    fn exists(&self) -> io::Result<bool>;
    /// Crea la tarea o reemplaza la que hubiera.
    fn register(&mut self, xml: &str) -> io::Result<()>;
    /// Quita la tarea. `true` si existía.
    fn remove(&mut self) -> io::Result<bool>;
}

/// Permisos de escritura de una ruta. En Windows lee la ACL ([`super::WinAcl`]).
pub trait PathAcl {
    /// `true` si, además de los administradores y el sistema, nadie (`Users`,
    /// `Authenticated Users`, `Everyone`) puede escribir, borrar o cambiar los permisos de
    /// `path`. Un error cuenta como «no se sabe»: se trata como no seguro.
    fn admin_only(&self, path: &Path, rigor: super::acl::Rigor) -> io::Result<bool>;
}

/// Por qué no se registró la tarea elevada.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotElevated {
    /// El proceso no tiene el token elevado.
    NoAdmin,
    /// El ejecutable o su carpeta los puede escribir quien no es administrador: elevarlo
    /// sería darle un salto de UAC a cualquier proceso del usuario.
    Writable(String),
    /// Sin `DOMINIO\usuario` el disparador valdría para cualquier usuario.
    NoUser,
}

/// Qué quedó registrado para arrancar con la sesión.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentInstall {
    /// Tarea programada con los privilegios más altos.
    Task { replaced: bool },
    /// La clave `Run`, con los privilegios del usuario, por el motivo dado.
    RunKey(InstallReport, NotElevated),
}

/// Registra el agente para arrancar con la sesión con los privilegios más altos solo si se
/// puede (`scheduler.is_elevated()`), se sabe el usuario y `launcher` y su carpeta solo los
/// escriben administradores (`acl`); si no, en la clave `Run` sin elevar. Con la tarea
/// registrada se quita el valor de `Run` (arrancaría una segunda copia); al caer a `Run` con
/// administrador se quita una tarea vieja que apuntara a otra ruta.
pub fn install_agent(
    scheduler: &mut dyn Scheduler,
    acl: &dyn PathAcl,
    autostart: &mut dyn Autostart,
    launcher: &Path,
    args: &RunArgs,
    user: Option<&str>,
) -> Result<AgentInstall, InstallError> {
    let elevated = scheduler.is_elevated();
    let user = user.filter(|u| !u.is_empty());
    let reason = match user {
        _ if !elevated => NotElevated::NoAdmin,
        None => NotElevated::NoUser,
        Some(_) => match not_admin_only(acl, launcher) {
            Some(why) => NotElevated::Writable(why),
            None => {
                let user = user.unwrap_or_default();
                let replaced = scheduler.exists().map_err(InstallError::Task)?;
                scheduler
                    .register(&task_xml(launcher, args, user))
                    .map_err(InstallError::Task)?;
                super::uninstall(autostart)?;
                return Ok(AgentInstall::Task { replaced });
            }
        },
    };
    if elevated {
        tracing::warn!(
            ?reason,
            "no se registra la tarea elevada: el agente arranca con la clave Run, sin elevar"
        );
        scheduler.remove().map_err(InstallError::Task)?;
    }
    let report = super::install(autostart, &command_line(launcher, args))?;
    Ok(AgentInstall::RunKey(report, reason))
}

/// El motivo por el que el ejecutable o alguna carpeta de su ruta no son solo de
/// administradores, si lo hay. Se recorre hasta la raíz: quien puede borrar o renombrar una
/// carpeta padre sustituye todo lo que cuelga de ella.
pub fn not_admin_only(acl: &dyn PathAcl, launcher: &Path) -> Option<String> {
    use super::acl::Rigor;
    let paths = launcher.ancestors().filter(|p| !p.as_os_str().is_empty());
    // El ejecutable y su carpeta, con la máscara completa (plantado de DLL); el resto, con la
    // de ancestros.
    for (i, path) in paths.enumerate() {
        let rigor = if i < 2 { Rigor::Full } else { Rigor::Ancestor };
        match acl.admin_only(path, rigor) {
            Ok(true) => {}
            Ok(false) => {
                return Some(format!(
                    "{} no es solo de administradores (escribible, de otro dueño o enlace)",
                    path.display()
                ));
            }
            Err(e) => return Some(format!("no se pudo leer la ACL de {}: {e}", path.display())),
        }
    }
    None
}

/// Quita los dos arranques. Devuelve `(tarea, clave Run)`: cuáles estaban registrados.
/// Quitar la tarea pide administrador, igual que registrarla.
pub fn uninstall_agent(
    scheduler: &mut dyn Scheduler,
    autostart: &mut dyn Autostart,
) -> Result<(bool, bool), InstallError> {
    // Se intentan las dos aunque una falle: la tarea elevada no puede quedar apuntando a una
    // ruta que ya no se controla.
    let task = scheduler.remove().map_err(InstallError::Task);
    let run = super::uninstall(autostart);
    Ok((task?, run?))
}

/// La definición de la tarea. `user` (`DOMINIO\usuario`) es quien inicia sesión y bajo cuya
/// cuenta corre; es obligatorio: sin él el disparador valdría para cualquier usuario.
pub fn task_xml(launcher: &Path, args: &RunArgs, user: &str) -> String {
    let full = command_line(launcher, args);
    // `Command` es solo el programa y `Arguments` lo demás: la línea sin el programa.
    let program = launcher.display().to_string();
    let arguments = full
        .strip_prefix(&format!("\"{program}\""))
        .unwrap_or_default()
        .trim();
    let user_id = format!("<UserId>{}</UserId>", escape(user));
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>Agente de Tandem: Mac y PC como un solo puesto.</Description>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
      {user_id}
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Tandem">
      {user_id}
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>HighestAvailable</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>true</StartWhenAvailable>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>7</Priority>
  </Settings>
  <Actions Context="Tandem">
    <Exec>
      <Command>{command}</Command>
      <Arguments>{arguments}</Arguments>
    </Exec>
  </Actions>
</Task>
"#,
        command = escape(&program),
        arguments = escape(arguments),
    )
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Doble de prueba de [`PathAcl`]: `writable` son las rutas que cualquier usuario escribe.
#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct MemAcl {
    pub writable: Vec<std::path::PathBuf>,
    pub fail: bool,
}

#[cfg(test)]
impl PathAcl for MemAcl {
    fn admin_only(&self, path: &Path, _rigor: super::acl::Rigor) -> io::Result<bool> {
        if self.fail {
            return Err(io::Error::other("acceso denegado"));
        }
        Ok(!self.writable.iter().any(|w| w == path))
    }
}

/// Doble de prueba de [`Scheduler`].
#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct MemScheduler {
    pub elevated: bool,
    pub task: Option<String>,
    pub fail: bool,
}

#[cfg(test)]
impl Scheduler for MemScheduler {
    fn is_elevated(&self) -> bool {
        self.elevated
    }

    fn exists(&self) -> io::Result<bool> {
        Ok(self.task.is_some())
    }

    fn register(&mut self, xml: &str) -> io::Result<()> {
        if self.fail {
            return Err(io::Error::other("Programador de tareas no disponible"));
        }
        self.task = Some(xml.to_owned());
        Ok(())
    }

    fn remove(&mut self) -> io::Result<bool> {
        if self.fail {
            return Err(io::Error::other("acceso denegado"));
        }
        Ok(self.task.take().is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::MemAutostart;

    const LAUNCHER: &str = "C:/Program Files/Tandem/tandemw.exe";
    const DIR: &str = "C:/Program Files/Tandem";

    fn install(
        scheduler: &mut MemScheduler,
        run: &mut MemAutostart,
        args: &RunArgs,
    ) -> Result<AgentInstall, InstallError> {
        install_with(scheduler, &MemAcl::default(), run, args, Some(r"PC\ana"))
    }

    fn install_with(
        scheduler: &mut MemScheduler,
        acl: &MemAcl,
        run: &mut MemAutostart,
        args: &RunArgs,
        user: Option<&str>,
    ) -> Result<AgentInstall, InstallError> {
        install_agent(scheduler, acl, run, Path::new(LAUNCHER), args, user)
    }

    fn elevated() -> MemScheduler {
        MemScheduler {
            elevated: true,
            ..Default::default()
        }
    }

    #[test]
    fn ejecutable_o_carpeta_escribibles_no_registran_la_tarea_y_usan_run() {
        for writable in [LAUNCHER, DIR] {
            let mut task = elevated();
            let acl = MemAcl {
                writable: vec![writable.into()],
                ..Default::default()
            };
            let mut run = MemAutostart::default();
            let got = install_with(
                &mut task,
                &acl,
                &mut run,
                &RunArgs::default(),
                Some("PC\\ana"),
            )
            .unwrap();
            let AgentInstall::RunKey(_, NotElevated::Writable(why)) = got else {
                panic!("se esperaba la clave Run por ruta escribible: {got:?}");
            };
            assert!(why.contains(writable), "{why}");
            assert_eq!(task.task, None, "no se registra con la ruta escribible");
            assert!(run.command.is_some());
        }
    }

    #[test]
    fn un_ancestro_escribible_aunque_sea_el_abuelo_no_registra_la_tarea() {
        // La raíz sale de `ancestors()` tal cual: `C:` en Linux, `C:/` en Windows.
        let root = Path::new(LAUNCHER)
            .ancestors()
            .filter(|p| !p.as_os_str().is_empty())
            .last()
            .unwrap()
            .to_path_buf();
        for writable in [Path::new("C:/Program Files").to_path_buf(), root] {
            let writable = writable.display().to_string();
            let mut task = elevated();
            let acl = MemAcl {
                writable: vec![writable.clone().into()],
                ..Default::default()
            };
            let mut run = MemAutostart::default();
            let got = install_with(
                &mut task,
                &acl,
                &mut run,
                &RunArgs::default(),
                Some("PC\\ana"),
            )
            .unwrap();
            let AgentInstall::RunKey(_, NotElevated::Writable(why)) = got else {
                panic!("se esperaba la clave Run por ancestro escribible: {got:?}");
            };
            assert!(why.contains(&writable), "{why}");
            assert_eq!(task.task, None);
        }
    }

    #[test]
    fn no_poder_leer_la_acl_cuenta_como_insegura() {
        let mut task = elevated();
        let acl = MemAcl {
            fail: true,
            ..Default::default()
        };
        let mut run = MemAutostart::default();
        let got = install_with(
            &mut task,
            &acl,
            &mut run,
            &RunArgs::default(),
            Some("PC\\ana"),
        )
        .unwrap();
        assert!(matches!(
            got,
            AgentInstall::RunKey(_, NotElevated::Writable(_))
        ));
        assert_eq!(task.task, None);
    }

    #[test]
    fn sin_usuario_no_se_registra_la_tarea() {
        for user in [None, Some("")] {
            let mut task = elevated();
            let mut run = MemAutostart::default();
            let got = install_with(
                &mut task,
                &MemAcl::default(),
                &mut run,
                &RunArgs::default(),
                user,
            )
            .unwrap();
            assert!(matches!(got, AgentInstall::RunKey(_, NotElevated::NoUser)));
            assert_eq!(task.task, None);
        }
    }

    #[test]
    fn al_caer_a_run_con_administrador_se_quita_la_tarea_vieja() {
        let mut task = MemScheduler {
            task: Some("vieja".into()),
            ..elevated()
        };
        let acl = MemAcl {
            writable: vec![LAUNCHER.into()],
            ..Default::default()
        };
        let mut run = MemAutostart::default();
        install_with(
            &mut task,
            &acl,
            &mut run,
            &RunArgs::default(),
            Some("PC\\ana"),
        )
        .unwrap();
        assert_eq!(task.task, None);
    }

    #[test]
    fn desinstalar_quita_la_clave_aunque_falle_la_tarea() {
        let mut task = MemScheduler {
            fail: true,
            task: Some("x".into()),
            ..elevated()
        };
        let mut run = MemAutostart {
            command: Some("y".into()),
            ..Default::default()
        };
        assert!(uninstall_agent(&mut task, &mut run).is_err());
        assert_eq!(run.command, None);
    }

    #[test]
    fn elevado_registra_la_tarea_y_quita_la_clave_run() {
        let mut task = MemScheduler {
            elevated: true,
            ..Default::default()
        };
        let mut run = MemAutostart {
            command: Some("viejo".into()),
            ..Default::default()
        };
        let got = install(&mut task, &mut run, &RunArgs::default()).unwrap();
        assert_eq!(got, AgentInstall::Task { replaced: false });
        assert!(task.task.is_some());
        assert_eq!(run.command, None, "no puede arrancar dos veces");
        // Reinstalar reemplaza la tarea.
        let got = install(&mut task, &mut run, &RunArgs::default()).unwrap();
        assert_eq!(got, AgentInstall::Task { replaced: true });
    }

    #[test]
    fn sin_administrador_queda_la_clave_run_y_la_tarea_no_se_toca() {
        let mut task = MemScheduler::default();
        let mut run = MemAutostart::default();
        let got = install(&mut task, &mut run, &RunArgs::default()).unwrap();
        let AgentInstall::RunKey(report, NotElevated::NoAdmin) = got else {
            panic!("se esperaba la clave Run");
        };
        assert!(report.autostart_changed);
        assert_eq!(
            run.command.as_deref(),
            Some(format!("\"{LAUNCHER}\"").as_str())
        );
        assert_eq!(task.task, None);
    }

    #[test]
    fn un_error_de_la_tarea_se_informa_y_no_quita_el_arranque_que_habia() {
        let mut task = MemScheduler {
            elevated: true,
            fail: true,
            ..Default::default()
        };
        let mut run = MemAutostart {
            command: Some("viejo".into()),
            ..Default::default()
        };
        let err = install(&mut task, &mut run, &RunArgs::default()).unwrap_err();
        assert!(matches!(err, InstallError::Task(_)), "{err}");
        assert_eq!(run.command.as_deref(), Some("viejo"));
    }

    #[test]
    fn desinstalar_quita_los_dos() {
        let mut task = MemScheduler {
            elevated: true,
            task: Some("x".into()),
            ..Default::default()
        };
        let mut run = MemAutostart {
            command: Some("y".into()),
            ..Default::default()
        };
        assert_eq!(uninstall_agent(&mut task, &mut run).unwrap(), (true, true));
        assert_eq!(
            uninstall_agent(&mut task, &mut run).unwrap(),
            (false, false)
        );
        task.fail = true;
        assert!(uninstall_agent(&mut task, &mut run).is_err());
    }

    #[test]
    fn el_xml_pide_los_privilegios_mas_altos_y_no_limita_al_agente() {
        let args = RunArgs {
            config: Some(r"D:\cfg\mi & tandem.toml".into()),
            ..RunArgs::default()
        };
        let xml = task_xml(Path::new(LAUNCHER), &args, r"PC\ana");
        assert!(xml.contains("<RunLevel>HighestAvailable</RunLevel>"));
        assert!(xml.contains("<ExecutionTimeLimit>PT0S</ExecutionTimeLimit>"));
        assert!(xml.contains("<DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>"));
        assert!(xml.contains("<LogonTrigger>"));
        assert!(xml.contains(r"<UserId>PC\ana</UserId>"));
        assert!(xml.contains(&format!("<Command>{LAUNCHER}</Command>")));
        // Los argumentos van aparte del programa y escapados.
        assert!(
            xml.contains(
                "<Arguments>--config &quot;D:\\cfg\\mi &amp; tandem.toml&quot;</Arguments>"
            ),
            "{xml}"
        );
        // El usuario está en el disparador y en el principal.
        assert_eq!(xml.matches("<UserId>").count(), 2);
    }
}
