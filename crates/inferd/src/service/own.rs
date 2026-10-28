//! The options that make a session its own: whose turn it carries (`asker`) and the model it
//! names for itself (`model`). Only a caller that may choose places (docket's daemons) may send
//! either; from anyone else the call is refused, not ignored.

use crate::peers::Caller;
use porter_dbus::Details;
use porter_dbus::inference::{OPTION_ASKER, OPTION_MODEL};
use porter_infer::{Asker, ModelRef};
use zbus::fdo;

/// What a session is on its own account.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Own {
    /// Whose turn the session carries; the person when the call names none.
    pub asker: Option<Asker>,
    /// The model the session names for itself, treated like a model the person named.
    pub model: Option<ModelRef>,
}

impl Own {
    /// Whose turn the session carries: the one named, else the person.
    pub fn asker(&self) -> Asker {
        self.asker.clone().unwrap_or(Asker::Person)
    }
}

/// The text value of `key`, when the call has it and it is a string.
fn text_of(options: &Details, key: &str) -> Option<fdo::Result<String>> {
    let value = options.get(key)?;
    Some(
        value
            .try_clone()
            .ok()
            .and_then(|value| String::try_from(value).ok())
            .ok_or_else(|| fdo::Error::InvalidArgs(format!("{key} is not a string"))),
    )
}

/// The `asker` and `model` of a call, when it names them. A caller that may not choose places
/// is refused if it sends either; a value that is not what the interface says is invalid args.
pub fn own_of(caller: &Caller, options: &Details) -> fdo::Result<Own> {
    let (asker, model) = (
        text_of(options, OPTION_ASKER),
        text_of(options, OPTION_MODEL),
    );
    if asker.is_none() && model.is_none() {
        return Ok(Own::default());
    }
    if !caller.may_choose_places() {
        return Err(fdo::Error::AccessDenied(
            "inferd: this caller may not name who asks or which model a session uses".into(),
        ));
    }
    let asker = asker
        .transpose()?
        .map(|text| {
            Asker::from_text(&text)
                .ok_or_else(|| fdo::Error::InvalidArgs("asker is not person or helper:<id>".into()))
        })
        .transpose()?;
    let model = model
        .transpose()?
        .map(|text| {
            ModelRef::parse(&text)
                .ok_or_else(|| fdo::Error::InvalidArgs("model is not <account>/<model>".into()))
        })
        .transpose()?;
    Ok(Own { asker, model })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peers::Role;
    use porter_core::{AppId, AppName, Isolation};
    use zbus::zvariant::{OwnedValue, Value};

    fn options(pairs: &[(&str, &str)]) -> Details {
        pairs
            .iter()
            .map(|(key, value)| {
                (
                    (*key).to_owned(),
                    OwnedValue::try_from(Value::from(*value)).expect("value"),
                )
            })
            .collect()
    }

    fn caller(role: Role) -> Caller {
        Caller {
            app: AppId {
                name: AppName::parse("org.quire.Companion").expect("app"),
                isolation: Isolation::Unsandboxed,
            },
            role,
        }
    }

    #[test]
    fn asker_and_model_are_read_for_a_placer_and_refused_to_anyone_else() {
        let sent = options(&[("asker", "helper:h-1"), ("model", "this-computer/qwen3-4b")]);
        let own = own_of(&caller(Role::Placer), &sent).expect("placer");
        assert_eq!(Some(own.asker()), Asker::from_text("helper:h-1"));
        assert_eq!(own.model, ModelRef::parse("this-computer/qwen3-4b"));
        assert!(matches!(
            own_of(&caller(Role::App), &sent),
            Err(fdo::Error::AccessDenied(_))
        ));
        assert_eq!(
            own_of(&caller(Role::App), &Details::new()),
            Ok(Own::default())
        );
        assert_eq!(Own::default().asker(), Asker::Person);
        for bad in [
            ("asker", "helper:"),
            ("asker", "nobody"),
            ("model", "no-slash"),
        ] {
            assert!(
                matches!(
                    own_of(&caller(Role::Placer), &options(&[bad])),
                    Err(fdo::Error::InvalidArgs(_))
                ),
                "{bad:?}"
            );
        }
    }
}
