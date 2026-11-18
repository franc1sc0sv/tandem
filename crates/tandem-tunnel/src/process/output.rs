//! La salida de un proceso (`output`) armada desde sus dos flujos, igual en la destino
//! (flujos propios) y en el origen (su espejo).

use tandem_proto::{FlowId, OutputCursor, ProcessOutput, TunnelProcess};

use crate::buffer::Read;

/// Lo más que devuelve un `output` por flujo: dos flujos más el resto caben en una trama
/// de la IPC (1 MiB).
pub const OUTPUT_MAX: usize = 400 * 1024;

/// Lo que da un flujo en una lectura.
pub(crate) struct FlowRead {
    pub read: Read,
    /// Posición del próximo byte que va a escribirse.
    pub end: u64,
    /// El flujo ya no recibe más.
    pub ended: bool,
}

struct Part {
    bytes: Vec<u8>,
    from: u64,
    next: u64,
    /// Ya se leyó todo y no va a haber más.
    finished: bool,
}

fn part(
    flow: FlowId,
    from: u64,
    max: usize,
    read: &mut impl FnMut(FlowId, u64, usize) -> Option<FlowRead>,
) -> Part {
    let nothing = |at| Part {
        bytes: Vec::new(),
        from: at,
        next: at,
        finished: false,
    };
    // Si lo pedido ya se descartó, se sigue desde lo que queda: una vez, porque la segunda
    // lectura ya empieza en lo más viejo que hay.
    let mut at = from;
    for _ in 0..2 {
        let Some(r) = read(flow, at, max) else {
            return nothing(at);
        };
        match r.read {
            Read::Data { offset, bytes } => {
                let next = offset + bytes.len() as u64;
                return Part {
                    bytes,
                    from: offset,
                    next,
                    finished: r.ended && next >= r.end,
                };
            }
            Read::Lost { to, .. } => at = to,
            Read::Pending => {
                return Part {
                    finished: r.ended && at >= r.end,
                    ..nothing(at)
                };
            }
        }
    }
    nothing(at)
}

/// `read(flujo, desde, máximo)` da el estado del flujo, o `None` si todavía no se conoce.
pub(crate) fn collect(
    process: TunnelProcess,
    from: OutputCursor,
    max: usize,
    mut read: impl FnMut(FlowId, u64, usize) -> Option<FlowRead>,
) -> ProcessOutput {
    let max = max.min(OUTPUT_MAX);
    let out = part(process.stdout, from.stdout, max, &mut read);
    let err = part(process.stderr, from.stderr, max, &mut read);
    ProcessOutput {
        done: !process.is_running() && out.finished && err.finished,
        process,
        from: OutputCursor {
            stdout: out.from,
            stderr: err.from,
        },
        next: OutputCursor {
            stdout: out.next,
            stderr: err.next,
        },
        stdout: out.bytes,
        stderr: err.bytes,
    }
}

#[cfg(test)]
mod tests {
    use tandem_proto::{ProcessId, ProcessState, TunnelId};

    use super::*;
    use crate::buffer::OutputBuffer;

    fn process(state: ProcessState) -> TunnelProcess {
        TunnelProcess {
            id: ProcessId(1),
            tunnel: TunnelId(1),
            command: "x".into(),
            shell: None,
            cwd: None,
            pid: None,
            stdout: FlowId(1),
            stderr: FlowId(2),
            started: 0,
            duration: None,
            state,
        }
    }

    fn flows(
        out: &OutputBuffer,
        err: &OutputBuffer,
        ended: bool,
    ) -> impl FnMut(FlowId, u64, usize) -> Option<FlowRead> {
        let (out, err) = (out.clone(), err.clone());
        move |f, from, max| {
            let b = if f == FlowId(1) { &out } else { &err };
            Some(FlowRead {
                read: b.read(from, max),
                end: b.end(),
                ended,
            })
        }
    }

    #[test]
    fn lee_cada_flujo_desde_su_posicion_y_marca_el_final() {
        let (mut out, mut err) = (OutputBuffer::new(100), OutputBuffer::new(100));
        out.push(b"hola\n");
        err.push(b"mal\n");
        let p = process(ProcessState::Exited { code: Some(0) });
        let o = collect(
            p.clone(),
            OutputCursor::default(),
            100,
            flows(&out, &err, true),
        );
        assert_eq!(
            (o.stdout.as_slice(), o.stderr.as_slice()),
            (&b"hola\n"[..], &b"mal\n"[..])
        );
        assert_eq!(
            o.next,
            OutputCursor {
                stdout: 5,
                stderr: 4
            }
        );
        assert!(o.done);
        // Desde donde terminó la lectura anterior no hay nada más, y sigue terminado.
        let o2 = collect(p, o.next, 100, flows(&out, &err, true));
        assert!(o2.stdout.is_empty() && o2.stderr.is_empty() && o2.done);
        assert_eq!(o2.from, o.next);
    }

    #[test]
    fn corriendo_o_con_salida_pendiente_no_esta_terminado() {
        let (mut out, err) = (OutputBuffer::new(100), OutputBuffer::new(100));
        out.push(b"abc");
        let o = collect(
            process(ProcessState::Running),
            OutputCursor::default(),
            2,
            flows(&out, &err, false),
        );
        // `max` limita cada flujo.
        assert_eq!(o.stdout, b"ab");
        assert!(!o.done);
        // Terminó el proceso pero queda salida por leer: todavía no.
        let o = collect(
            process(ProcessState::Killed),
            OutputCursor::default(),
            2,
            flows(&out, &err, true),
        );
        assert!(!o.done);
    }

    #[test]
    fn lo_descartado_se_salta_y_se_avisa_en_from() {
        let (mut out, err) = (OutputBuffer::new(4), OutputBuffer::new(4));
        out.push(b"0123456789");
        let o = collect(
            process(ProcessState::Running),
            OutputCursor::default(),
            100,
            flows(&out, &err, false),
        );
        assert_eq!(o.stdout, b"6789");
        assert_eq!((o.from.stdout, o.next.stdout), (6, 10));
    }

    #[test]
    fn un_flujo_desconocido_no_da_nada() {
        let o = collect(
            process(ProcessState::Running),
            OutputCursor {
                stdout: 7,
                stderr: 3,
            },
            100,
            |_, _, _| None,
        );
        assert_eq!(
            (o.from, o.next),
            (
                OutputCursor {
                    stdout: 7,
                    stderr: 3
                },
                OutputCursor {
                    stdout: 7,
                    stderr: 3
                }
            )
        );
        assert!(!o.done);
    }
}
