//! The wire word of a closed set: the name serde gives a unit variant, so an enum that is sent as
//! one word on a bus, in a file or on a command line derives `Serialize` and `Deserialize` with
//! `#[serde(rename_all = "snake_case")]` and writes the table of words once, in the derive.
//!
//! ```
//! use porter_ids::word::{from_word, word};
//!
//! #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
//! #[serde(rename_all = "snake_case")]
//! enum State {
//!     Ready,
//!     NeedsSignIn,
//! }
//!
//! assert_eq!(word(&State::NeedsSignIn), "needs_sign_in");
//! assert_eq!(from_word::<State>("ready"), Some(State::Ready));
//! assert_eq!(from_word::<State>("Ready"), None);
//! ```

use serde::de::value::{Error as WordError, StrDeserializer};
use serde::de::{DeserializeOwned, IntoDeserializer};
use serde::ser::{self, Impossible, Serialize, Serializer};
use std::fmt;

/// The word `value` is sent as.
///
/// # Panics
/// When `value` does not serialize as a unit variant: only enums whose variants carry nothing
/// have a word, and a type that is not one is a mistake at the call site.
pub fn word<T: Serialize + ?Sized>(value: &T) -> &'static str {
    match value.serialize(WordOnly) {
        Ok(word) => word,
        Err(NotAWord) => panic!("a wire word is a unit variant of an enum"),
    }
}

/// The value the word `text` names; `None` for any other text.
pub fn from_word<T: DeserializeOwned>(text: &str) -> Option<T> {
    let word: StrDeserializer<'_, WordError> = text.into_deserializer();
    T::deserialize(word).ok()
}

/// The refusal of a value that is not a unit variant.
#[derive(Debug)]
struct NotAWord;

impl fmt::Display for NotAWord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("not a unit variant")
    }
}

impl std::error::Error for NotAWord {}

impl ser::Error for NotAWord {
    fn custom<T: fmt::Display>(_: T) -> Self {
        NotAWord
    }
}

/// A serializer that takes the name of a unit variant and refuses everything else.
struct WordOnly;

/// Refuses each of the listed serializer methods.
macro_rules! refuse {
    ($($method:ident($($argument:ty),*);)*) => {
        $(fn $method(self, $(_: $argument),*) -> Result<&'static str, NotAWord> {
            Err(NotAWord)
        })*
    };
}

impl Serializer for WordOnly {
    type Ok = &'static str;
    type Error = NotAWord;
    type SerializeSeq = Impossible<&'static str, NotAWord>;
    type SerializeTuple = Impossible<&'static str, NotAWord>;
    type SerializeTupleStruct = Impossible<&'static str, NotAWord>;
    type SerializeTupleVariant = Impossible<&'static str, NotAWord>;
    type SerializeMap = Impossible<&'static str, NotAWord>;
    type SerializeStruct = Impossible<&'static str, NotAWord>;
    type SerializeStructVariant = Impossible<&'static str, NotAWord>;

    fn serialize_unit_variant(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
    ) -> Result<&'static str, NotAWord> {
        Ok(variant)
    }

    refuse! {
        serialize_bool(bool);
        serialize_i8(i8);
        serialize_i16(i16);
        serialize_i32(i32);
        serialize_i64(i64);
        serialize_u8(u8);
        serialize_u16(u16);
        serialize_u32(u32);
        serialize_u64(u64);
        serialize_f32(f32);
        serialize_f64(f64);
        serialize_char(char);
        serialize_str(&str);
        serialize_bytes(&[u8]);
        serialize_none();
        serialize_unit();
        serialize_unit_struct(&'static str);
    }

    fn serialize_some<T: Serialize + ?Sized>(self, _: &T) -> Result<&'static str, NotAWord> {
        Err(NotAWord)
    }

    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        _: &T,
    ) -> Result<&'static str, NotAWord> {
        Err(NotAWord)
    }

    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: &T,
    ) -> Result<&'static str, NotAWord> {
        Err(NotAWord)
    }

    fn serialize_seq(self, _: Option<usize>) -> Result<Self::SerializeSeq, NotAWord> {
        Err(NotAWord)
    }

    fn serialize_tuple(self, _: usize) -> Result<Self::SerializeTuple, NotAWord> {
        Err(NotAWord)
    }

    fn serialize_tuple_struct(
        self,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeTupleStruct, NotAWord> {
        Err(NotAWord)
    }

    fn serialize_tuple_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeTupleVariant, NotAWord> {
        Err(NotAWord)
    }

    fn serialize_map(self, _: Option<usize>) -> Result<Self::SerializeMap, NotAWord> {
        Err(NotAWord)
    }

    fn serialize_struct(
        self,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeStruct, NotAWord> {
        Err(NotAWord)
    }

    fn serialize_struct_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeStructVariant, NotAWord> {
        Err(NotAWord)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    #[serde(rename_all = "snake_case")]
    enum Sample {
        Stopped,
        NeedsSignIn,
    }

    #[test]
    fn a_unit_variant_is_its_renamed_word_and_back() {
        assert_eq!(word(&Sample::Stopped), "stopped");
        assert_eq!(word(&Sample::NeedsSignIn), "needs_sign_in");
        assert_eq!(from_word("needs_sign_in"), Some(Sample::NeedsSignIn));
        assert_eq!(from_word::<Sample>("NeedsSignIn"), None);
        assert_eq!(from_word::<Sample>(""), None);
    }

    #[test]
    #[should_panic(expected = "unit variant")]
    fn a_value_that_is_not_a_unit_variant_has_no_word() {
        let _ = word(&7_u8);
    }
}
