//! Cutting a value into pieces that each fit one entry.

use crate::Limit;

/// `value` cut so that every piece fits `limit` in UTF-16 code units, never inside a character.
pub(crate) fn text(value: &str, limit: Limit) -> Vec<&str> {
    let Limit::Utf16Units(most) = limit else {
        return vec![value];
    };
    // A character is at most two units, so a limit under two could never place one.
    let most = most.max(2);
    let mut out = Vec::new();
    let mut start = 0;
    let mut units = 0;
    for (at, c) in value.char_indices() {
        if units + c.len_utf16() > most {
            out.push(&value[start..at]);
            start = at;
            units = 0;
        }
        units += c.len_utf16();
    }
    out.push(&value[start..]);
    out
}

/// `value` cut so that every piece fits `limit`, at two bytes to a UTF-16 code unit.
pub(crate) fn bytes(value: &[u8], limit: Limit) -> Vec<&[u8]> {
    let Limit::Utf16Units(most) = limit else {
        return vec![value];
    };
    if value.is_empty() {
        return vec![value];
    }
    value.chunks(most.max(1).saturating_mul(2)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const EIGHT: Limit = Limit::Utf16Units(8);

    #[test]
    fn a_value_is_cut_where_it_fits_and_never_inside_a_character() {
        let cases: &[(&str, Limit, &[&str])] = &[
            ("{\"a\":1}", Limit::None, &["{\"a\":1}"]),
            ("{\"a\":1}", EIGHT, &["{\"a\":1}"]),
            ("0123456789", EIGHT, &["01234567", "89"]),
            // Each of these is two UTF-16 units, as it is to Windows.
            ("😀😀😀😀😀", EIGHT, &["😀😀😀😀", "😀"]),
            ("1234567😀", EIGHT, &["1234567", "😀"]),
            ("", EIGHT, &[""]),
        ];
        for (value, limit, expect) in cases {
            assert_eq!(text(value, *limit), *expect, "{value:?} at {limit:?}");
            for piece in text(value, *limit) {
                if let Limit::Utf16Units(most) = limit {
                    assert!(piece.encode_utf16().count() <= *most, "{piece:?}");
                }
            }
        }
    }

    #[test]
    fn bytes_are_cut_at_two_to_a_unit() {
        let value: Vec<u8> = (0..40).collect();
        let cut = bytes(&value, EIGHT);
        assert_eq!(cut.iter().map(|p| p.len()).collect::<Vec<_>>(), [16, 16, 8]);
        assert_eq!(cut.concat(), value);
        assert_eq!(bytes(&[], EIGHT), [&[] as &[u8]]);
        assert_eq!(bytes(&value, Limit::None), [&value[..]]);
    }
}
