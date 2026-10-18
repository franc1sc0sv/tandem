//! El estado inicial real del puesto (T-085): los monitores no dicen su entrada, así que
//! al arrancar Tandem no sabe lo que muestra cada uno hasta «Aplicar modo» o hasta que el
//! usuario lo diga.
//!
//! Con el reloj de tokio pausado, como el resto de los escenarios.

use tandem_core::{Machine, Mode, MonitorId};
use tandem_sim::{Desk, Outcome, Power, SimError, TxnRun};

fn m34() -> MonitorId {
    "m34".into()
}

fn m32() -> MonitorId {
    "m32".into()
}

async fn committed(d: &Desk, on: Machine, run: &TxnRun) {
    assert_eq!(run.outcome(), Some(Outcome::Committed), "{:#?}", run.events);
    d.check_from(on).await.unwrap();
    d.check_geometry().unwrap();
}

fn switched(d: &Desk) -> Vec<MonitorId> {
    d.world.ddc_writes().into_iter().map(|(m, _)| m).collect()
}

/// Primer arranque: ninguna de las dos sabe nada, y antes alguien pasó el 34" a la PC desde
/// su menú. Un plan sobre lo inventado («todo en la Mac») no haría nada; «Aplicar modo»
/// manda la entrada de los dos y el estado queda confirmado, igual a lo que se mandó.
#[tokio::test(start_paused = true)]
async fn aplicar_modo_sin_estado_manda_todo_y_confirma() {
    let d = Desk::builder().unconfirmed().build().await.unwrap();
    d.world.switch_by_hand(&m34(), Machine::Pc);
    for m in [Machine::Mac, Machine::Pc] {
        assert!(!d.agent(m).state().is_confirmed(), "{m}");
    }
    let run = d.apply_mode(Machine::Mac, Mode::SoloMac).await.unwrap();
    committed(&d, Machine::Mac, &run).await;
    // Hubo que despertar a la PC, la que cambia las entradas por DDC.
    assert_eq!(d.world.power_of(Machine::Pc), Power::On);
    assert_eq!(switched(&d), [m34(), m32()]);
    for m in [m34(), m32()] {
        assert_eq!(d.world.shows(&m), Some(Machine::Mac));
    }
    let state = d.mac.state();
    assert!(state.is_confirmed() && state.doubt.is_empty(), "{state:?}");
    // La otra recibe el estado confirmado.
    assert!(d.pc_agrees().await.unwrap().is_confirmed());
}

/// Sin confirmar, el primer cambio de modo normal funciona como «Aplicar modo»: manda
/// todas las entradas, no solo las que cambiarían según lo inventado.
#[tokio::test(start_paused = true)]
async fn el_primer_cambio_sin_confirmar_manda_todas_las_entradas() {
    let d = Desk::builder().unconfirmed().build().await.unwrap();
    // El 32" quedó en la PC de otra sesión; Tandem cree que está en la Mac.
    d.world.switch_by_hand(&m32(), Machine::Pc);
    let shared = Mode::Shared([(m34(), Machine::Pc)].into());
    let run = d.set_mode(Machine::Mac, shared.clone()).await.unwrap();
    committed(&d, Machine::Mac, &run).await;
    assert_eq!(switched(&d), [m34(), m32()]);
    assert_eq!(d.world.shows(&m34()), Some(Machine::Pc));
    assert_eq!(d.world.shows(&m32()), Some(Machine::Mac));
    assert!(d.mac.state().is_confirmed());
    // Confirmado, el mismo modo ya no manda nada.
    let run = d.set_mode(Machine::Mac, shared).await.unwrap();
    committed(&d, Machine::Mac, &run).await;
    assert_eq!(switched(&d).len(), 2);
}

/// Lo mismo desde la PC, que es la que cambia las entradas: no hace falta despertar a la
/// Mac para Solo Windows.
#[tokio::test(start_paused = true)]
async fn aplicar_modo_desde_la_pc_sin_estado() {
    let d = Desk::builder()
        .principal(Machine::Pc)
        .unconfirmed()
        .build()
        .await
        .unwrap();
    d.world.switch_by_hand(&m32(), Machine::Mac);
    let run = d.apply_mode(Machine::Pc, Mode::SoloPc).await.unwrap();
    committed(&d, Machine::Pc, &run).await;
    assert_eq!(d.world.power_of(Machine::Mac), Power::Off);
    assert_eq!(switched(&d), [m34(), m32()]);
    assert!(d.pc.state().is_confirmed());
}

/// Corregir un monitor desde la escena («en realidad muestra la PC»): cambia el estado
/// sin ningún paso DDC, cada escritorio se pone al día y el estado llega a la otra.
#[tokio::test(start_paused = true)]
async fn corregir_un_monitor_no_manda_ddc_y_llega_a_la_otra() {
    let d = Desk::new().await.unwrap();
    let run = d
        .set_mode(Machine::Mac, Mode::Shared([(m34(), Machine::Pc)].into()))
        .await
        .unwrap();
    committed(&d, Machine::Mac, &run).await;
    d.pc_agrees().await.unwrap();
    let writes = d.world.ddc_writes();
    let calls = d.world.calls().len();

    // Alguien pasa el 32" a la PC desde su menú; Tandem sigue creyéndolo en la Mac.
    d.world.switch_by_hand(&m32(), Machine::Pc);
    assert!(d.check_from(Machine::Mac).await.is_err());

    // Se corrige desde la UI de la PC.
    let run = d
        .set_shown(Machine::Pc, [(m32(), Some(Machine::Pc))])
        .await
        .unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Committed), "{:#?}", run.events);
    assert_eq!(d.world.ddc_writes(), writes, "ninguna escritura DDC");
    let new_calls = &d.world.calls()[calls..];
    assert!(
        !new_calls
            .iter()
            .any(|(_, s)| matches!(s, tandem_core::Step::SwitchInput { .. })),
        "{new_calls:?}"
    );
    assert_eq!(d.pc.state().owner[&m32()], Machine::Pc);
    // La Mac recibe lo corregido.
    let agreed = d.agrees(Machine::Mac).await.unwrap();
    assert_eq!(agreed.owner[&m32()], Machine::Pc);
    // Lo dicho por el usuario no se comprueba con ninguna entrada, así que a la Mac no se
    // le quita nada (T-087): el 32" sigue en su escritorio como fantasma, y es lo único
    // que difiere. El PC sí lo tiene.
    let SimError::Mismatch(diffs) = d.check_from(Machine::Mac).await.unwrap_err() else {
        panic!("se esperaba una diferencia")
    };
    assert_eq!(diffs.len(), 1, "{diffs:?}");
    assert!(diffs[0].contains("escritorio de mac"), "{diffs:?}");
    // «Aplicar modo» sí manda la entrada y después quita el fantasma.
    let both_pc = Mode::Shared([(m34(), Machine::Pc), (m32(), Machine::Pc)].into());
    let run = d.apply_mode(Machine::Mac, both_pc).await.unwrap();
    committed(&d, Machine::Mac, &run).await;
}

/// Responder «¿qué muestra ahora?» tras un arranque sin estado, en cualquiera de las dos
/// máquinas: se confirma sin DDC y se sincroniza.
#[tokio::test(start_paused = true)]
async fn responder_que_muestra_cada_monitor_confirma_sin_ddc() {
    let d = Desk::builder().unconfirmed().build().await.unwrap();
    d.world.switch_by_hand(&m34(), Machine::Pc);
    // La PC está apagada: el 34" en su entrada está en negro.
    assert_eq!(d.world.shows(&m34()), None);
    let run = d
        .set_shown(Machine::Mac, [(m34(), None), (m32(), Some(Machine::Mac))])
        .await
        .unwrap();
    assert_eq!(run.outcome(), Some(Outcome::Committed), "{:#?}", run.events);
    assert!(d.world.ddc_writes().is_empty());
    let state = d.mac.state();
    assert!(state.is_confirmed(), "{state:?}");
    assert!(
        state.in_doubt(&m34()),
        "el negro se manda en el próximo cambio"
    );
    // El próximo cambio le manda la entrada al 34" y lo trae.
    let run = d.set_mode(Machine::Mac, Mode::SoloMac).await.unwrap();
    committed(&d, Machine::Mac, &run).await;
    assert_eq!(switched(&d), [m34()]);
    assert_eq!(d.world.shows(&m34()), Some(Machine::Mac));
    assert!(d.mac.state().doubt.is_empty());
}
