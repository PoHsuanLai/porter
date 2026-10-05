use super::*;

const INFO: &str = "[Application]\nname=org.example.Photos\nruntime=runtime/org.gnome.Platform/x86_64/47\n\n[Instance]\ninstance-id=1234567\napp-path=/x\n";

#[test]
fn a_flatpak_info_names_the_app_and_the_instance() {
    assert_eq!(
        flatpak_facts(INFO),
        Some(SandboxFacts::Flatpak {
            app: "org.example.Photos".to_owned(),
            instance: "1234567".to_owned()
        })
    );
}

#[test]
fn a_flatpak_info_without_an_application_name_is_not_understood() {
    for text in ["", "[Instance]\ninstance-id=1\n", "name=org.example.A\n"] {
        assert_eq!(flatpak_facts(text), None, "{text:?}");
    }
}

#[test]
fn a_name_in_another_section_is_not_the_application() {
    let text = "[Context]\nname=org.example.Evil\n[Application]\nname=org.example.Good\n";
    assert_eq!(
        flatpak_facts(text),
        Some(SandboxFacts::Flatpak {
            app: "org.example.Good".to_owned(),
            instance: String::new()
        })
    );
}
