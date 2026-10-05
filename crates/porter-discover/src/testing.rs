//! Fakes of the two seams, and the document builders the tests share.

use crate::dns::{Dns, DnsFault, MxRecord, SrvRecord};
use porter_http::{Http, HttpError, HttpRequest, HttpResponse, Method, Status};
use porter_provider::DomainName;
use std::collections::HashMap;
use std::sync::Mutex;

pub(crate) const ADDRESS: &str = "someone@example.test";

pub(crate) fn name(text: &str) -> DomainName {
    DomainName::parse(text).unwrap_or_else(|e| panic!("{text}: {e}"))
}

/// A document with the given servers inside one provider.
pub(crate) fn doc(servers: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<clientConfig version="1.1">
  <emailProvider id="example.test">
    <domain>example.test</domain>
    <displayName>Example</displayName>
    {servers}
  </emailProvider>
</clientConfig>"#
    )
}

pub(crate) fn server(
    kind: &str,
    host: &str,
    port: u16,
    socket: &str,
    user: &str,
    auth: &[&str],
) -> String {
    let auth: String = auth
        .iter()
        .map(|a| format!("<authentication>{a}</authentication>"))
        .collect();
    let tag = if kind == "smtp" {
        "outgoingServer"
    } else {
        "incomingServer"
    };
    format!(
        "<{tag} type=\"{kind}\">
           <hostname>{host}</hostname>
           <port>{port}</port>
           <socketType>{socket}</socketType>
           <username>{user}</username>
           {auth}
         </{tag}>"
    )
}

pub(crate) fn srv(priority: u16, weight: u16, port: u16, target: &str) -> SrvRecord {
    SrvRecord {
        priority,
        weight,
        port,
        target: name(target),
    }
}

pub(crate) fn mx(preference: u16, host: &str) -> MxRecord {
    MxRecord {
        preference,
        host: name(host),
    }
}

pub(crate) fn respond(status: u16, body: &str) -> HttpResponse {
    HttpResponse {
        status: Status(status),
        headers: Vec::new(),
        body: body.as_bytes().to_vec(),
    }
}

/// An `Http` answering from a table keyed by `METHOD url`; a request not in it is unreachable.
/// Every request is recorded.
#[derive(Default)]
pub(crate) struct Table {
    answers: HashMap<String, HttpResponse>,
    pub(crate) seen: Mutex<Vec<String>>,
}

impl Table {
    pub(crate) fn with(mut self, method: Method, url: &str, response: HttpResponse) -> Self {
        self.answers
            .insert(format!("{} {url}", method.token()), response);
        self
    }

    pub(crate) fn get(self, url: &str, response: HttpResponse) -> Self {
        self.with(Method::Get, url, response)
    }

    pub(crate) fn seen(&self) -> Vec<String> {
        self.seen.lock().expect("seen").clone()
    }
}

impl Http for Table {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, HttpError> {
        let key = format!("{} {}", request.method.token(), request.url);
        self.seen.lock().expect("seen").push(key.clone());
        self.answers
            .get(&key)
            .cloned()
            .ok_or(HttpError::Unreachable)
    }
}

/// DNS from a table. A name not in it has no records.
#[derive(Default)]
pub(crate) struct Records {
    pub(crate) srv: HashMap<String, Vec<SrvRecord>>,
    pub(crate) mx: HashMap<String, Vec<MxRecord>>,
    /// Every query fails as if the network were down.
    pub(crate) down: bool,
    pub(crate) asked: Mutex<Vec<String>>,
}

impl Dns for Records {
    async fn srv(&self, name: &str) -> Result<Vec<SrvRecord>, DnsFault> {
        self.asked
            .lock()
            .expect("asked")
            .push(format!("SRV {name}"));
        match self.down {
            true => Err(DnsFault::Unreachable),
            false => self.srv.get(name).cloned().ok_or(DnsFault::NoRecords),
        }
    }

    async fn mx(&self, domain: &DomainName) -> Result<Vec<MxRecord>, DnsFault> {
        self.asked
            .lock()
            .expect("asked")
            .push(format!("MX {domain}"));
        match self.down {
            true => Err(DnsFault::Unreachable),
            false => self
                .mx
                .get(domain.as_str())
                .cloned()
                .ok_or(DnsFault::NoRecords),
        }
    }
}
