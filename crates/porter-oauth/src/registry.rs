//! Which client id this build presents to an issuer: the ones shipped, with a person's own
//! (written only by Settings) laid over them.

use porter_provider::{ClientChannel, ClientEntry, ClientsFile, Issuer};

/// The clients of this build, shipped and the person's own.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClientRegistry {
    shipped: Vec<ClientEntry>,
    own: Vec<ClientEntry>,
}

impl ClientRegistry {
    /// The shipped clients with the person's own laid over them.
    pub fn layered(shipped: ClientsFile, own: ClientsFile) -> Self {
        Self {
            shipped: shipped.clients,
            own: own.clients,
        }
    }

    /// The client for `issuer` on `channel`: the person's own if they registered one, else the
    /// shipped one.
    pub fn lookup(&self, issuer: Issuer, channel: ClientChannel) -> Option<&ClientEntry> {
        self.own
            .iter()
            .chain(&self.shipped)
            .find(|c| c.issuer == issuer && c.channel == channel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_provider::parse_clients;

    fn file(text: &str) -> ClientsFile {
        parse_clients(text).expect("parses")
    }

    #[test]
    fn the_persons_own_client_wins_and_others_fall_through() {
        let shipped = file(
            "[[client]]\nissuer = \"google\"\nchannel = \"stable\"\nclient_id = \"shipped-g\"\n\
             [[client]]\nissuer = \"microsoft\"\nchannel = \"stable\"\nclient_id = \"shipped-ms\"\n",
        );
        let own =
            file("[[client]]\nissuer = \"google\"\nchannel = \"stable\"\nclient_id = \"mine\"\n");
        let registry = ClientRegistry::layered(shipped, own);
        let id = |issuer, channel| {
            registry
                .lookup(issuer, channel)
                .map(|c| c.client_id.0.as_str())
        };
        assert_eq!(id(Issuer::Google, ClientChannel::Stable), Some("mine"));
        assert_eq!(
            id(Issuer::Microsoft, ClientChannel::Stable),
            Some("shipped-ms")
        );
        assert_eq!(id(Issuer::Microsoft, ClientChannel::Beta), None);
        assert_eq!(id(Issuer::Dropbox, ClientChannel::Stable), None);
    }
}
