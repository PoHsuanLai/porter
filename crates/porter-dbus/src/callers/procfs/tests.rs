use super::*;

#[test]
fn a_flatpak_info_names_its_app_and_instance_and_one_without_a_name_names_nothing() {
    let cases = [
        (
            "[Application]\nname=org.example.Photos\nruntime=runtime/org.gnome.Platform\n\n[Instance]\ninstance-id=123\n",
            Some(("org.example.Photos", "123")),
        ),
        // Spaces around the `=`, and no instance section (an older Flatpak).
        (
            "[Application]\nname = org.example.Mail\n",
            Some(("org.example.Mail", "")),
        ),
        // A name in another section is not the application's.
        ("[Runtime]\nname=org.gnome.Platform\n", None),
        ("[Application]\nname=\n", None),
        ("", None),
    ];
    for (info, want) in cases {
        let got = flatpak_facts(info);
        let want = want.map(|(app, instance)| SandboxFacts::Flatpak {
            app: app.to_owned(),
            instance: instance.to_owned(),
        });
        assert_eq!(got, want, "{info:?}");
    }
}

#[test]
fn a_fixture_main_pid_is_read_from_its_units_file_and_nothing_else() {
    let root = std::env::temp_dir().join(format!("porter-dbus-units-{}", std::process::id()));
    std::fs::create_dir_all(root.join("units")).expect("units");
    std::fs::write(root.join("units").join("inferd.service"), "4242\n").expect("pid");
    std::fs::write(root.join("units").join("bad.service"), "not a pid").expect("bad");
    assert_eq!(fixture_main_pid(&root, "inferd.service"), Some(4242));
    assert_eq!(fixture_main_pid(&root, "bad.service"), None);
    assert_eq!(fixture_main_pid(&root, "syncd.service"), None);
    assert_eq!(fixture_main_pid(&root, "../units/inferd.service"), None);
    let _ = std::fs::remove_dir_all(root);
}
