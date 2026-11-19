//! T-209 with two agents: one side still has a `tandem.toml` with the Spanish ids, the other one
//! is already in English. They converge on the English desk and stop there: the rename is not a
//! change, so neither keeps rewriting it for the other.

use std::time::Duration;

use tandem_core::Machine;
use tandem_sim::{Desk, Outcome};
use tandemd::config::AgentConfig;

const LIMIT: Duration = Duration::from_secs(600);

const MACHINES: &str = "[machines.a]\nos = \"macos\"\nname = \"MacBook\"\ncolor = \"#7aa7e6\"\n\
                        [machines.b]\nos = \"windows\"\nname = \"PC\"\ncolor = \"#a99be8\"\n";

/// A desk saved by a release before T-209.
fn spanish(rev: u64) -> String {
    format!(
        "desk_rev = {rev}\ndesk_origin = \"a\"\n{MACHINES}\
         [[screens]]\nmachine = \"a\"\nid = \"pantalla-1\"\nx = 0\ny = 0\nwidth = 1512\nheight = 982\n\
         [[scenes]]\nid = \"trabajo\"\nname = \"Trabajo\"\nicon = \"puesto\"\n\
         [[scenes]]\nid = \"solo-a\"\nname = \"Solo MacBook\"\nrequire_on = \"a\"\n\
         [[scenes]]\nid = \"solo-b\"\nname = \"Solo PC\"\nrequire_on = \"b\"\n\
         [[cables]]\nmonitor = \"lc34-01\"\n[cables.a]\nscreen = \"pantalla-1\"\n"
    )
}

/// A desk saved by this release, with the fingerprint the agent writes (not edited by hand).
fn english(rev: u64, work: &str) -> String {
    let body = format!(
        "{MACHINES}\
         [[screens]]\nmachine = \"a\"\nid = \"screen-1\"\nx = 0\ny = 0\nwidth = 1512\nheight = 982\n\
         [[scenes]]\nid = \"work\"\nname = \"{work}\"\nicon = \"desk\"\n\
         [[scenes]]\nid = \"only-a\"\nname = \"Only MacBook\"\nrequire_on = \"a\"\n\
         [[scenes]]\nid = \"only-b\"\nname = \"Only PC\"\nrequire_on = \"b\"\n\
         [[cables]]\nmonitor = \"lc34-01\"\n[cables.a]\nscreen = \"screen-1\"\n"
    );
    let desk = AgentConfig::from_toml(&format!("machine = \"b\"\n{body}"))
        .unwrap()
        .desk;
    let sum = tandemd::desk_sync::fingerprint(&desk);
    format!("desk_rev = {rev}\ndesk_origin = \"a\"\ndesk_sum = \"{sum}\"\n{body}")
}

fn scene_ids(d: &Desk, m: Machine) -> Vec<String> {
    d.agent(m)
        .desk()
        .scenes
        .iter()
        .map(|s| s.id.clone())
        .collect()
}

/// Whichever side has the newer desk, both end with the same English one and the same version,
/// and it stays that way.
#[tokio::test(start_paused = true)]
async fn an_old_desk_and_a_new_one_converge_without_a_rewrite_loop() {
    // (A's file, B's file, the name "Work" ends with, the final version)
    let cases = [
        (spanish(6), english(5, "Office"), "Work", 6),
        (spanish(5), english(7, "Office"), "Office", 7),
    ];
    for (a, b, work, rev) in cases {
        let d = Desk::builder()
            .toml(Machine::A, &a)
            .toml(Machine::B, &b)
            .build()
            .await
            .unwrap();
        // The Spanish file is read in English already.
        assert_eq!(scene_ids(&d, Machine::A), ["work", "only-a", "only-b"]);
        let run = d.power_on(Machine::B).await.unwrap();
        assert_eq!(run.outcome(), Some(Outcome::Committed), "{:#?}", run.events);

        let mut rx = d.pc.subscribe_desk();
        tokio::time::timeout(LIMIT, rx.wait_for(|desk| *desk == d.mac.desk()))
            .await
            .expect("the desks did not converge")
            .unwrap();
        let stamp = d.mac.desk_sync().stamp;
        assert_eq!(stamp.rev, rev, "the rename is not a change");
        for _ in 0..3 {
            d.settle().await;
        }
        for m in Machine::ALL {
            let agent = d.agent(m);
            assert_eq!(agent.desk_sync().stamp, stamp, "{m}: the version moved");
            assert_eq!(agent.desk(), d.mac.desk());
            assert_eq!(scene_ids(&d, m), ["work", "only-a", "only-b"]);
            let desk = agent.desk();
            assert_eq!(desk.scenes[0].name, work);
            assert_eq!(desk.scenes[0].icon, "desk");
            assert_eq!(desk.screens[0].id, "screen-1");
            assert_eq!(desk.cables[0].a.screen.as_deref(), Some("screen-1"));
        }
    }
}
