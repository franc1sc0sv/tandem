use std::collections::VecDeque;

use proptest::prelude::*;

use super::*;
use crate::content::Image;

fn cfg() -> SyncConfig {
    SyncConfig {
        inline_max: 64,
        chunk_len: 100,
        max_len: 10_000,
    }
}

/// Sin reloj de pared: el orden lo dan solo Lamport y el hash.
fn side(focused: bool) -> ClipSync {
    ClipSync::with_clock(cfg(), focused, || 0)
}

fn text(s: &str) -> ClipContent {
    ClipContent::text(s)
}

/// Más grande que `inline_max`: va perezoso.
fn big(fill: u8) -> ClipContent {
    ClipContent {
        image: Some(Image {
            width: 10,
            height: 10,
            rgba: vec![fill; 400],
        }),
        ..ClipContent::default()
    }
}

fn sends(actions: Vec<Action>) -> Vec<ClipMsg> {
    actions
        .into_iter()
        .map(|a| match a {
            Action::Send(m) => m,
            Action::Write(c) => panic!("se esperaba solo envíos, hubo Write({c:?})"),
        })
        .collect()
}

fn one_send(actions: Vec<Action>) -> ClipMsg {
    let mut m = sends(actions);
    assert_eq!(m.len(), 1, "{m:?}");
    m.remove(0)
}

fn written(actions: &[Action]) -> Vec<ClipContent> {
    actions
        .iter()
        .filter_map(|a| match a {
            Action::Write(c) => Some(c.clone()),
            Action::Send(_) => None,
        })
        .collect()
}

/// Todos los trozos de la transferencia en curso.
fn drain_chunks(s: &mut ClipSync) -> Vec<ClipMsg> {
    let mut out = Vec::new();
    while let Some(c) = s.next_chunk() {
        out.push(c);
    }
    assert!(!s.is_sending());
    out
}

#[test]
fn copia_chica_va_dentro_del_anuncio_y_el_otro_la_escribe() {
    let mut mac = side(false);
    let mut pc = side(false);
    let offer = one_send(mac.local_changed(Some(text("hola"))));
    let ClipMsg::Offer { content, len, .. } = &offer else {
        panic!("{offer:?}")
    };
    assert_eq!(content.as_ref(), Some(&text("hola")));
    assert!(*len > 0);
    // Aunque la PC no tenga el foco: lo chico se escribe al llegar.
    let acts = pc.on_message(offer);
    assert_eq!(acts, vec![Action::Write(text("hola"))]);
    assert_eq!(pc.stats().written, 1);
    assert_eq!(mac.stats().offers_sent, 1);
}

#[test]
fn la_propia_escritura_no_se_anuncia_de_vuelta() {
    let mut mac = side(false);
    let mut pc = side(false);
    let offer = one_send(mac.local_changed(Some(text("eco"))));
    assert_eq!(written(&pc.on_message(offer)), vec![text("eco")]);
    // El sistema avisa que el portapapeles de la PC cambió: es lo que escribimos.
    assert!(pc.local_changed(Some(text("eco"))).is_empty());
    assert_eq!(pc.stats().offers_sent, 0);
    // Un cambio que no cambia nada tampoco.
    assert!(mac.local_changed(Some(text("eco"))).is_empty());
    assert_eq!(mac.stats().offers_sent, 1);
}

#[test]
fn una_oferta_con_lo_que_ya_tenemos_no_se_escribe() {
    let mut mac = side(true);
    let mut pc = side(true);
    let _ = pc.local_changed(Some(big(3)));
    let offer = one_send(mac.local_changed(Some(big(3))));
    assert!(pc.on_message(offer).is_empty());
    assert!(!pc.has_pending());
    assert_eq!(pc.stats().written, 0);
}

#[test]
fn copia_grande_se_transfiere_al_tomar_el_foco() {
    let mut mac = side(true);
    let mut pc = side(false);
    let content = big(9);
    let offer = one_send(mac.local_changed(Some(content.clone())));
    let ClipMsg::Offer {
        id,
        content: None,
        len,
        ..
    } = offer
    else {
        panic!("{offer:?}")
    };
    // Sin el foco solo queda anotada.
    assert!(pc.on_message(offer).is_empty());
    assert!(pc.has_pending());

    let request = one_send(pc.set_focus(true));
    assert_eq!(request, ClipMsg::Request { id });
    // Volver a tener el foco no lo pide dos veces.
    assert!(pc.set_focus(true).is_empty());

    assert!(mac.on_message(request).is_empty());
    assert!(mac.is_sending());
    let chunks = drain_chunks(&mut mac);
    assert!(chunks.len() > 1);
    let mut total = 0;
    let mut acts = Vec::new();
    for (i, c) in chunks.iter().enumerate() {
        let ClipMsg::Chunk { data, last, .. } = c else {
            panic!("{c:?}")
        };
        assert!(data.len() <= cfg().chunk_len);
        assert_eq!(*last, i == chunks.len() - 1);
        total += data.len();
        acts.extend(pc.on_message(c.clone()));
    }
    assert_eq!(total as u64, len);
    assert_eq!(written(&acts), vec![content.clone()]);
    assert!(!pc.has_pending());
    // Y la escritura no vuelve como eco.
    assert!(pc.local_changed(Some(content)).is_empty());
}

#[test]
fn con_el_foco_se_pide_apenas_llega_el_anuncio() {
    let mut mac = side(false);
    let mut pc = side(true);
    let offer = one_send(mac.local_changed(Some(big(1))));
    let ClipMsg::Offer { id, .. } = offer else {
        panic!()
    };
    assert_eq!(one_send(pc.on_message(offer)), ClipMsg::Request { id });
}

#[test]
fn pedido_de_una_copia_vieja_responde_no_disponible() {
    let mut mac = side(false);
    let mut pc = side(false);
    let old = one_send(mac.local_changed(Some(big(1))));
    let ClipMsg::Offer { id: old_id, .. } = old else {
        panic!()
    };
    let _ = pc.on_message(old);
    // La Mac copia otra cosa antes de que el anuncio nuevo llegue a la PC.
    let newer = one_send(mac.local_changed(Some(text("nuevo"))));
    let request = one_send(pc.set_focus(true));
    let reply = one_send(mac.on_message(request));
    assert_eq!(reply, ClipMsg::Unavailable { id: old_id });
    assert!(!mac.is_sending());
    assert!(pc.on_message(reply).is_empty());
    assert!(!pc.has_pending());
    assert_eq!(written(&pc.on_message(newer)), vec![text("nuevo")]);
}

#[test]
fn copiar_a_mitad_de_una_transferencia_la_corta() {
    let mut mac = side(false);
    let mut pc = side(true);
    let offer = one_send(mac.local_changed(Some(big(1))));
    let request = one_send(pc.on_message(offer));
    let _ = mac.on_message(request);
    let first = mac.next_chunk().unwrap();
    let _ = pc.on_message(first);
    let newer = one_send(mac.local_changed(Some(big(2))));
    assert!(!mac.is_sending());
    assert_eq!(mac.next_chunk(), None);
    // El anuncio nuevo reemplaza la transferencia a medias del otro lado.
    let ClipMsg::Offer { id, .. } = newer else {
        panic!()
    };
    assert_eq!(one_send(pc.on_message(newer)), ClipMsg::Request { id });
    let _ = mac.on_message(ClipMsg::Request { id });
    let mut acts = Vec::new();
    for c in drain_chunks(&mut mac) {
        acts.extend(pc.on_message(c));
    }
    assert_eq!(written(&acts), vec![big(2)]);
}

#[test]
fn una_copia_local_reemplaza_lo_pendiente_del_otro_lado() {
    let mut mac = side(false);
    let mut pc = side(false);
    let offer = one_send(mac.local_changed(Some(big(1))));
    let _ = pc.on_message(offer);
    assert!(pc.has_pending());
    let _ = pc.local_changed(Some(text("mío")));
    assert!(!pc.has_pending());
    // Al tomar el foco no se pide lo viejo.
    assert!(pc.set_focus(true).is_empty());
}

#[test]
fn trozos_de_algo_no_pedido_se_ignoran() {
    let mut pc = side(true);
    let acts = pc.on_message(ClipMsg::Chunk {
        id: 5,
        data: vec![1, 2, 3],
        last: true,
    });
    assert!(acts.is_empty());
    assert_eq!(pc.stats(), SyncStats::default());
}

/// Lleva una oferta grande hasta tener la transferencia pedida.
fn requested(mac: &mut ClipSync, pc: &mut ClipSync, content: ClipContent) -> (u64, Vec<u8>) {
    let offer = one_send(mac.local_changed(Some(content.clone())));
    let ClipMsg::Offer { id, .. } = offer else {
        panic!()
    };
    let _ = one_send(pc.on_message(offer));
    (id, tandem_proto::encode(&content).unwrap())
}

#[test]
fn transferencia_corrupta_se_descarta() {
    let mut mac = side(false);
    let mut pc = side(true);
    let (id, mut bytes) = requested(&mut mac, &mut pc, big(4));
    let n = bytes.len();
    bytes[n - 1] ^= 0xFF;
    let acts = pc.on_message(ClipMsg::Chunk {
        id,
        data: bytes,
        last: true,
    });
    assert!(acts.is_empty());
    assert_eq!(pc.stats().rejected, 1);
    assert!(!pc.has_pending());
}

#[test]
fn transferencia_mas_larga_o_mas_corta_que_lo_anunciado_se_descarta() {
    let mut mac = side(false);
    let mut pc = side(true);
    let (id, mut bytes) = requested(&mut mac, &mut pc, big(4));
    bytes.push(0);
    assert!(
        pc.on_message(ClipMsg::Chunk {
            id,
            data: bytes,
            last: false,
        })
        .is_empty()
    );
    assert_eq!(pc.stats().rejected, 1);
    assert!(!pc.has_pending());

    let (id, mut bytes) = requested(&mut mac, &mut pc, big(5));
    bytes.pop();
    assert!(
        pc.on_message(ClipMsg::Chunk {
            id,
            data: bytes,
            last: true,
        })
        .is_empty()
    );
    assert_eq!(pc.stats().rejected, 2);
    assert_eq!(pc.stats().written, 0);
}

#[test]
fn ofertas_invalidas_se_descartan() {
    let mut pc = side(true);
    // Contenido que no es el de su hash.
    let acts = pc.on_message(ClipMsg::Offer {
        id: 1,
        hash: text("a").hash(),
        len: 3,
        content: Some(text("b")),
    });
    assert!(acts.is_empty());
    // Más grande que el máximo.
    let acts = pc.on_message(ClipMsg::Offer {
        id: 2,
        hash: text("a").hash(),
        len: cfg().max_len as u64 + 1,
        content: None,
    });
    assert!(acts.is_empty());
    assert!(!pc.has_pending());
    assert_eq!(pc.stats().rejected, 2);
    assert_eq!(pc.stats().written, 0);
}

#[test]
fn una_copia_que_supera_el_maximo_no_se_anuncia() {
    let mut mac = side(false);
    let huge = ClipContent {
        image: Some(Image {
            width: 100,
            height: 100,
            rgba: vec![0; 40_000],
        }),
        ..ClipContent::default()
    };
    assert!(mac.local_changed(Some(huge)).is_empty());
    assert_eq!(mac.stats().offers_sent, 0);
}

#[test]
fn portapapeles_vacio_o_sin_formatos_no_se_anuncia_y_corta_lo_ofrecido() {
    let mut mac = side(false);
    let offer = one_send(mac.local_changed(Some(big(1))));
    let ClipMsg::Offer { id, .. } = offer else {
        panic!()
    };
    // El usuario copia archivos: no se sincronizan.
    assert!(mac.local_changed(None).is_empty());
    assert_eq!(
        one_send(mac.on_message(ClipMsg::Request { id })),
        ClipMsg::Unavailable { id }
    );
}

#[test]
fn copias_cruzadas_terminan_iguales_en_los_dos_lados() {
    for (a, b) in [
        (text("x"), text("y")),
        (big(1), big(2)),
        (text("x"), big(2)),
    ] {
        let mut mac = side(true);
        let mut pc = side(true);
        // Las dos copias salen antes de que llegue la del otro.
        let from_mac = one_send(mac.local_changed(Some(a.clone())));
        let from_pc = one_send(pc.local_changed(Some(b.clone())));
        let winner = if a.hash() > b.hash() { &a } else { &b };
        let mut model = Model::new(mac, pc);
        model.to_pc.push_back(from_mac);
        model.to_mac.push_back(from_pc);
        model.mac.clip = Some(a.clone());
        model.pc.clip = Some(b.clone());
        model.settle(1000);
        assert_eq!(model.mac.clip.as_ref(), Some(winner));
        assert_eq!(model.pc.clip.as_ref(), Some(winner));
    }
}

#[test]
fn copia_hecha_despues_de_ver_la_del_otro_gana_siempre() {
    // Sea cual sea el orden de los hashes, la copia que vio la otra no es cruzada.
    for (a, b) in [(text("x"), text("y")), (text("y"), text("x"))] {
        let mut mac = side(false);
        let mut pc = side(false);
        let offer = one_send(mac.local_changed(Some(a.clone())));
        let _ = pc.on_message(offer);
        let reply = one_send(pc.local_changed(Some(b.clone())));
        assert_eq!(written(&mac.on_message(reply)), vec![b]);
    }
}

#[test]
fn al_empezar_se_anuncia_lo_copiado_sin_enlace_y_se_reanuncia_lo_vigente() {
    let mut mac = side(false);
    // Sin cambios y sin nada copiado: nada.
    assert!(mac.start(None).is_empty());
    // Copiado mientras no había enlace.
    let first = one_send(mac.start(Some(text("a"))));
    let ClipMsg::Offer { id, .. } = first else {
        panic!()
    };
    // Enlace nuevo, mismo contenido: se vuelve a anunciar la misma copia.
    let again = one_send(mac.start(Some(text("a"))));
    assert!(matches!(again, ClipMsg::Offer { id: i, content: Some(_), .. } if i == id));
    // Sin poder leer el portapapeles, también.
    assert_eq!(one_send(mac.resume()), again);
    // Si lo vigente vino del otro lado, no es nuestro: no se anuncia.
    let mut pc = side(false);
    let _ = pc.on_message(again);
    assert!(pc.start(Some(text("a"))).is_empty());
}

#[test]
fn lo_pendiente_sobrevive_al_cambio_de_enlace_y_se_vuelve_a_pedir() {
    let mut mac = side(false);
    let mut pc = side(true);
    let offer = one_send(mac.local_changed(Some(big(7))));
    let request = one_send(pc.on_message(offer));
    let _ = mac.on_message(request.clone());
    let first = mac.next_chunk().unwrap();
    let _ = pc.on_message(first);
    // Se cae el enlace a mitad de la transferencia.
    let mac_start = sends(mac.start(Some(big(7))));
    assert!(!mac.is_sending());
    let pc_start = sends(pc.start(None));
    assert_eq!(pc_start, vec![request.clone()]);
    // El reanuncio de la Mac llega y no duplica el pedido... sino que lo renueva.
    let mut model = Model::new(mac, pc);
    model.mac.clip = Some(big(7));
    model.to_pc.extend(mac_start);
    model.to_mac.extend(pc_start);
    model.settle(1000);
    assert_eq!(model.pc.clip, Some(big(7)));
    assert_eq!(model.pc.sync.stats().written, 1);
}

#[test]
fn las_copias_de_un_agente_reiniciado_ganan_por_el_reloj_de_pared() {
    let mut mac = side(false);
    let mut pc = side(false);
    let offer = one_send(mac.local_changed(Some(text("x"))));
    let _ = pc.on_message(offer);
    let offer = one_send(pc.local_changed(Some(text("y"))));
    let _ = mac.on_message(offer);
    // La PC se reinicia y olvida todo: sin reloj de pared, su copia nueva
    // tendría un id menor que el vigente y la Mac la ignoraría.
    let mut forgetful = side(false);
    let stale = one_send(forgetful.local_changed(Some(text("z"))));
    assert!(mac.clone().on_message(stale).is_empty());
    let mut restarted = ClipSync::with_clock(cfg(), false, || 1_000_000);
    let offer = one_send(restarted.local_changed(Some(text("z"))));
    assert_eq!(written(&mac.on_message(offer)), vec![text("z")]);
}

#[test]
fn chunk_len_y_inline_se_ajustan_a_la_trama() {
    let s = ClipSync::new(
        SyncConfig {
            inline_max: usize::MAX,
            chunk_len: 0,
            max_len: 1,
        },
        false,
    );
    assert!(s.cfg.inline_max < MAX_FRAME_LEN);
    assert_eq!(s.cfg.chunk_len, 1);
}

// --- Modelo de dos agentes con colas, para las propiedades ---

struct Agent {
    sync: ClipSync,
    /// Lo que tiene el portapapeles del sistema.
    clip: Option<ClipContent>,
}

struct Model {
    mac: Agent,
    pc: Agent,
    to_mac: VecDeque<ClipMsg>,
    to_pc: VecDeque<ClipMsg>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Who {
    Mac,
    Pc,
}

impl Model {
    fn new(mac: ClipSync, pc: ClipSync) -> Self {
        Self {
            mac: Agent {
                sync: mac,
                clip: None,
            },
            pc: Agent {
                sync: pc,
                clip: None,
            },
            to_mac: VecDeque::new(),
            to_pc: VecDeque::new(),
        }
    }

    fn parts(&mut self, who: Who) -> (&mut Agent, &mut VecDeque<ClipMsg>) {
        match who {
            Who::Mac => (&mut self.mac, &mut self.to_pc),
            Who::Pc => (&mut self.pc, &mut self.to_mac),
        }
    }

    /// Aplica acciones como el driver: un `Write` cambia el portapapeles y el
    /// sistema avisa del cambio, que tiene que salir sin anuncio (sin eco).
    fn apply(&mut self, who: Who, actions: Vec<Action>) {
        let (agent, outbox) = self.parts(who);
        for a in actions {
            match a {
                Action::Send(m) => outbox.push_back(m),
                Action::Write(c) => {
                    agent.clip = Some(c.clone());
                    let echo = agent.sync.local_changed(Some(c));
                    assert!(echo.is_empty(), "eco: {echo:?}");
                }
            }
        }
        // Los trozos salen de a uno, como en el driver.
        while let Some(c) = agent.sync.next_chunk() {
            outbox.push_back(c);
        }
    }

    fn copy(&mut self, who: Who, content: ClipContent) {
        let (agent, _) = self.parts(who);
        agent.clip = Some(content.clone());
        let acts = agent.sync.local_changed(Some(content));
        self.apply(who, acts);
    }

    fn focus(&mut self, who: Who, focused: bool) {
        let (agent, _) = self.parts(who);
        let acts = agent.sync.set_focus(focused);
        self.apply(who, acts);
    }

    /// Entrega el siguiente mensaje hacia `who`; `false` si no había.
    fn deliver(&mut self, who: Who) -> bool {
        let msg = match who {
            Who::Mac => self.to_mac.pop_front(),
            Who::Pc => self.to_pc.pop_front(),
        };
        let Some(msg) = msg else { return false };
        let acts = match who {
            Who::Mac => self.mac.sync.on_message(msg),
            Who::Pc => self.pc.sync.on_message(msg),
        };
        self.apply(who, acts);
        true
    }

    /// Enlace nuevo: lo que estaba en vuelo se pierde.
    fn restart(&mut self) {
        self.to_mac.clear();
        self.to_pc.clear();
        for who in [Who::Mac, Who::Pc] {
            let (agent, _) = self.parts(who);
            let clip = agent.clip.clone();
            let acts = agent.sync.start(clip);
            self.apply(who, acts);
        }
    }

    /// Entrega todo; el número de mensajes tiene que ser finito (sin eco
    /// infinito).
    fn settle(&mut self, max: usize) {
        let mut n = 0;
        while self.deliver(Who::Mac) | self.deliver(Who::Pc) {
            n += 1;
            assert!(n < max, "los mensajes no se acaban");
        }
    }
}

#[derive(Clone, Debug)]
enum Op {
    Copy(Who, ClipContent),
    Focus(Who),
    Deliver(Who),
    Restart,
}

fn who() -> impl Strategy<Value = Who> {
    prop_oneof![Just(Who::Mac), Just(Who::Pc)]
}

fn clip() -> impl Strategy<Value = ClipContent> {
    // Pocos valores, para que se repitan (eco, ofertas iguales).
    prop_oneof![
        (0u8..4).prop_map(|i| text(&format!("t{i}"))),
        (0u8..3).prop_map(big),
    ]
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        3 => (who(), clip()).prop_map(|(w, c)| Op::Copy(w, c)),
        2 => who().prop_map(Op::Focus),
        6 => who().prop_map(Op::Deliver),
        1 => Just(Op::Restart),
    ]
}

proptest! {
    /// Cualquier secuencia de copias, cambios de foco, entregas parciales y
    /// cambios de enlace (perdiendo lo que estaba en vuelo) termina con los dos
    /// portapapeles iguales una vez que los dos tuvieron el foco, sin eco y con un
    /// número finito de mensajes.
    #[test]
    fn los_dos_lados_convergen_sin_eco(ops in proptest::collection::vec(op(), 1..60)) {
        let mut m = Model::new(side(false), side(true));
        let mut copied = false;
        for op in ops {
            match op {
                Op::Copy(w, c) => {
                    copied = true;
                    m.copy(w, c);
                }
                Op::Focus(w) => {
                    // El foco está en una sola máquina.
                    let other = if w == Who::Mac { Who::Pc } else { Who::Mac };
                    m.focus(other, false);
                    m.focus(w, true);
                }
                Op::Deliver(w) => {
                    m.deliver(w);
                }
                Op::Restart => m.restart(),
            }
        }
        m.settle(10_000);
        for w in [Who::Mac, Who::Pc, Who::Mac] {
            let other = if w == Who::Mac { Who::Pc } else { Who::Mac };
            m.focus(other, false);
            m.focus(w, true);
            m.settle(10_000);
        }
        prop_assert_eq!(&m.mac.clip, &m.pc.clip);
        prop_assert_eq!(m.mac.clip.is_some(), copied);
        prop_assert_eq!(m.mac.sync.stats().rejected + m.pc.sync.stats().rejected, 0);
    }
}
