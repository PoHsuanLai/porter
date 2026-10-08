use super::*;

#[test]
fn a_letter_is_one_or_two_visible_characters() {
    for ok in ["A", "Cx", "Ol", "É", "ÉA"] {
        assert_eq!(MarkLetter::parse(ok).expect(ok).as_str(), ok);
    }
    for bad in ["", " ", "ABC", "A ", "\n", "\u{7}", "A\tB"] {
        assert_eq!(
            MarkLetter::parse(bad),
            Err(MarkFaceError::Letter(bad.to_owned())),
            "{bad:?}"
        );
    }
}

#[test]
fn a_colour_is_hash_and_six_hex_digits_stored_upper_case() {
    assert_eq!(
        MarkColour::parse("#d97757").expect("lower").as_str(),
        "#D97757"
    );
    assert_eq!(MarkColour::parse("#D97757"), MarkColour::parse("#d97757"));
    assert_eq!(
        MarkColour::parse("#000000").expect("black").as_str(),
        "#000000"
    );
    for bad in [
        "D97757",
        "#D9775",
        "#D977570",
        "#D97757FF",
        "#FFF",
        "red",
        "#GGGGGG",
        "",
        "#",
        "#é97757",
    ] {
        assert_eq!(
            MarkColour::parse(bad),
            Err(MarkFaceError::Colour(bad.to_owned())),
            "{bad:?}"
        );
    }
}

#[test]
fn serde_refuses_a_bad_letter_or_colour_and_keeps_the_text() {
    let face: MarkFace =
        serde_json::from_str(r##"{"letter":"Cx","colour":"#10a37f"}"##).expect("face");
    assert_eq!(face.letter.as_str(), "Cx");
    assert_eq!(face.colour.as_str(), "#10A37F");
    assert_eq!(
        serde_json::to_string(&face).expect("json"),
        r##"{"letter":"Cx","colour":"#10A37F"}"##
    );
    for bad in [
        r##"{"letter":"","colour":"#10A37F"}"##,
        r##"{"letter":"ABC","colour":"#10A37F"}"##,
        r##"{"letter":"A","colour":"#FFF"}"##,
        r##"{"letter":"A","colour":"red"}"##,
        r##"{"letter":"A"}"##,
    ] {
        assert!(serde_json::from_str::<MarkFace>(bad).is_err(), "{bad}");
    }
}
