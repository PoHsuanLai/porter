//! `ai.attached.my_network`: whether the person's other machines count as this computer for a floor
//! that says "this computer".

use super::*;
use crate::config::InferdConfig;
use porter_core::{DataClass, Locality};
use porter_infer::{Floor, LocalOnly};

fn settings(text: &str) -> (Settings, Vec<String>) {
    let config = InferdConfig::from_toml(text).expect("config");
    let resolved = resolve(&config);
    (resolved.settings, resolved.rejected)
}

#[test]
fn the_row_is_off_by_default_and_reads_on_and_names_a_value_it_does_not_know() {
    assert_eq!(settings("").0.my_network, MyNetwork::Off);
    let (on, rejected) = settings("[ai.attached]\nmy_network = \"on\"\n");
    assert_eq!((on.my_network, rejected), (MyNetwork::On, vec![]));
    let (off, rejected) = settings("[ai.attached]\nmy_network = \"off\"\n");
    assert_eq!((off.my_network, rejected), (MyNetwork::Off, vec![]));
    let (odd, rejected) = settings("[ai.attached]\nmy_network = \"sometimes\"\n");
    assert_eq!(odd.my_network, MyNetwork::Off);
    assert_eq!(rejected, [MY_NETWORK.to_owned()]);
}

#[test]
fn on_lets_an_on_device_floor_reach_my_machines_and_nothing_else_moves() {
    let text = "[ai]\nlocal_only = \"off\"\n[ai.floor]\nmail = \"on_device\"\nnotes = \"local_network\"\n\
                files = \"anywhere\"\n[ai.attached]\nmy_network = \"on\"\n";
    let (on, _) = settings(text);
    let (off, _) = settings(&text.replace("my_network = \"on\"", "my_network = \"off\""));
    let machines = Locality::LocalNetwork;
    let cloud = Locality::Cloud { region: None };
    let table = [
        // class, floor without the row, floor with it
        (DataClass::Mail, Floor::OnDevice, Floor::LocalNetwork),
        (DataClass::Notes, Floor::LocalNetwork, Floor::LocalNetwork),
        (DataClass::Files, Floor::Anywhere, Floor::Anywhere),
        // The proposed default of a class the file does not mention.
        (DataClass::Photos, Floor::OnDevice, Floor::LocalNetwork),
        (DataClass::Public, Floor::Anywhere, Floor::Anywhere),
    ];
    for (class, without, with) in table {
        assert_eq!(off.routing_policy().floor(class), without, "{class:?} off");
        assert_eq!(on.routing_policy().floor(class), with, "{class:?} on");
        // The floor the person set is untouched: only routing reads the other one.
        assert_eq!(on.policy.floor(class), without, "{class:?} as set");
    }
    // Another machine of theirs passes only where the row says so; the cloud never does.
    assert!(
        !off.routing_policy()
            .floor(DataClass::Mail)
            .admits(&machines)
    );
    assert!(on.routing_policy().floor(DataClass::Mail).admits(&machines));
    assert!(!on.routing_policy().floor(DataClass::Mail).admits(&cloud));
    assert_eq!(on.routing_policy().local_only, LocalOnly::Off);
    assert_eq!(off.routing_policy(), off.policy);
}
