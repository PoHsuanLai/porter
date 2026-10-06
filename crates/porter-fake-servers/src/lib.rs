//! Test-only fake servers for the accounts program (PLAN §6 row W2e, §8): an OAuth issuer with a
//! scripted browser, IMAP and SMTP with a scratch CA, a Nextcloud, a plain DAV server, autoconfig
//! and well-known answers, a DNS answer table behind `porter-discover`'s `Dns` seam, and Ollama
//! and OpenAI-compatible model lists.
//!
//! Each server listens on a loopback port (or a Unix socket in a scratch directory where the
//! protocol allows it), is built from the provider files the product ships (`shipped`), has its
//! endpoints rewritten to its own address (`FakeServer::rewrite`), and records what it received on
//! a handle so a test can assert on it ("the IMAP fake saw LOGIN from the relay, never from the
//! app"). The servers implement `porter_fake::FakeServer`; this crate is never a dependency of a
//! non-test crate. Nothing here touches a network beyond loopback.

pub mod autoconfig;
pub mod browser;
pub mod dav;
pub mod dav_server;
pub mod http;
pub mod imap;
pub mod llm_api;
pub mod mail;
pub mod models;
pub mod net;
pub mod nextcloud;
pub mod oauth;
pub mod seen;
pub mod shipped;
pub mod smtp;
pub mod tls;

pub use autoconfig::{AutoconfigHandle, FakeAutoconfig, FakeDns, Route, autoconfig_xml};
pub use browser::{Visit, follow};
pub use dav_server::{DavHandle, FakeDav};
pub use http::{Hit, Request, Response};
pub use imap::{FakeImap, Message, mailbox};
pub use llm_api::{Auth, Call, FakeLlmApi, LlmApiHandle};
pub use mail::{Accounts, Attempt, MailEvent, MailHandle, Mechanism, Secret};
pub use models::{FakeModels, ModelDef, ModelsHandle, Wire};
pub use net::{Bind, Peer};
pub use nextcloud::{FakeNextcloud, LoginPolicy, NextcloudHandle};
pub use oauth::{Consent, FakeIssuer, IssuerEvent, IssuerHandle, TokenResult};
pub use seen::{Running, Seen};
pub use smtp::FakeSmtp;
