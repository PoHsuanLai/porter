//! The rule the four relays (IMAP, POP3, SMTP, ManageSieve) share when they choose how to present
//! a credential: a token goes by `XOAUTH2`, a password by `PLAIN`, and nothing else is ever sent.

use crate::fault::RelayFault;
use porter_core::RelayAuth;

/// The SASL mechanism a credential goes by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Sasl {
    /// `PLAIN`, for a password.
    Plain,
    /// `XOAUTH2`, for an access token.
    Xoauth2,
}

/// The mechanism to present `auth` by, given whether the server offers `PLAIN` and `XOAUTH2`:
/// `Protocol` when it offers the one the credential needs no way to. An anonymous credential, and
/// a kind a newer porter adds, are refused here: never sent as anonymous.
pub(crate) fn pick(auth: &RelayAuth, plain: bool, xoauth2: bool) -> Result<Sasl, RelayFault> {
    match auth {
        RelayAuth::AccessToken(_) if xoauth2 => Ok(Sasl::Xoauth2),
        RelayAuth::Password(_) if plain => Ok(Sasl::Plain),
        _ => Err(RelayFault::Protocol),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_core::SecretText;

    #[test]
    fn a_token_goes_by_xoauth2_and_a_password_by_plain_and_nothing_else_is_sent() {
        let token = RelayAuth::AccessToken(SecretText::new("t"));
        let password = RelayAuth::Password(SecretText::new("p"));
        assert_eq!(pick(&token, true, true), Ok(Sasl::Xoauth2));
        assert_eq!(pick(&token, true, false), Err(RelayFault::Protocol));
        assert_eq!(pick(&password, true, true), Ok(Sasl::Plain));
        assert_eq!(pick(&password, false, true), Err(RelayFault::Protocol));
        assert_eq!(
            pick(&RelayAuth::Anonymous, true, true),
            Err(RelayFault::Protocol)
        );
    }
}
