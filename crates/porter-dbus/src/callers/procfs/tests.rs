use super::*;

#[test]
fn a_flatpak_scope_names_its_app_with_the_id_unescaped() {
    let cases = [
        (
            "app-flatpak-org.example.Photos-5.scope",
            Some("org.example.Photos"),
        ),
        (
            "app-flatpak-org.example.My\\x2dApp-1234.scope",
            Some("org.example.My-App"),
        ),
        ("app-flatpak-org.example.Photos.scope", None),
        ("app-flatpak-notaname-5.scope", None),
        ("app-flatpak-org.example.A-5.service", None),
        ("app-gnome-org.example.A-5.scope", None),
        ("app-flatpak-org.example.A\\xZZ-5.scope", None),
    ];
    for (leaf, want) in cases {
        assert_eq!(
            flatpak_scope_app(leaf).as_ref().map(AppName::as_str),
            want,
            "{leaf}"
        );
    }
}
