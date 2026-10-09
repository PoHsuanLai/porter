//! Starting the fakes: the one place that knows which `porter-fake-servers` fake each
//! `porter-rig-servers` flag means, what is planted in it, and what `rig.json` says of it.
//! Everything listens on `127.0.0.1` on an ephemeral port (the OS gives ports from 32768 up,
//! never below 1024), and everything of the rig lives in one scratch directory.

use crate::planted::{
    API_KEY, DAV_PASSWORD, DAV_USER, GOOGLE_CLIENT, GOOGLE_CLIENT_SECRET, MAIL_PASSWORD, MAIL_USER,
    NEXTCLOUD_APP_PASSWORD, OAUTH_ACCESS_TOKEN, OAUTH_CLIENT, OAUTH_REFRESH_TOKEN, OAUTH_SCOPE,
};
use crate::rigfile::{GoogleAt, HttpAt, LlmApiAt, MailAt, OauthAt, OllamaAt, RigFile};
use porter_core::Tls;
use porter_fake_servers::imap::mailbox;
use porter_fake_servers::mail::Accounts;
use porter_fake_servers::net::Bind;
use porter_fake_servers::{
    DavHandle, FakeDav, FakeGraph, FakeImap, FakeIssuer, FakeLlmApi, FakeModels, FakeNextcloud,
    FakePop3, FakeSmtp, Google, GraphHandle, IssuerHandle, LlmApiHandle, MailHandle, ModelDef,
    ModelsHandle, NextcloudHandle, Running, Wire, llm_api::Auth, tls,
};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

/// Which fakes to start, and how the mail ones are secured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// The scratch directory (made if missing).
    pub dir: PathBuf,
    /// An IMAP server.
    pub imap: bool,
    /// An SMTP server.
    pub smtp: bool,
    /// A POP3 server.
    pub pop3: bool,
    /// A plain DAV server.
    pub dav: bool,
    /// A Nextcloud.
    pub nextcloud: bool,
    /// The OAuth issuer.
    pub oauth: bool,
    /// The Graph drive, which accepts the issuer's tokens (so it starts the issuer too).
    pub graph: bool,
    /// A fake Google: its own issuer (Google's style, wanting the planted application secret)
    /// and the account APIs that accept its tokens.
    pub google: bool,
    /// The `expires_in` of the issuer's access tokens, in seconds (none: the issuer's 3600).
    pub token_lifetime_s: Option<u64>,
    /// An Ollama.
    pub ollama: bool,
    /// An LLM API that wants a bearer key.
    pub llm_api: bool,
    /// How the mail servers are secured.
    pub tls: Tls,
}

/// The name of the TLS mode as `rig.json` spells it.
fn tls_word(tls: Tls) -> &'static str {
    match tls {
        Tls::Implicit => "implicit",
        Tls::StartTls => "start_tls",
        Tls::Plain => "plain",
    }
}

/// The models the fake Ollama lists.
fn ollama_models() -> Vec<ModelDef> {
    vec![
        ModelDef::chat("llama3.2:3b", 8192),
        ModelDef::embedding("nomic-embed-text", 2048),
    ]
}

/// The fake Ollama, stoppable and startable again at the port it first had.
#[derive(Debug)]
pub struct Ollama {
    port: u16,
    running: Mutex<Option<Running<ModelsHandle>>>,
}

impl Ollama {
    /// Binds the first time, on an ephemeral port.
    async fn first() -> io::Result<Self> {
        let running = FakeModels::start(Wire::Ollama, ollama_models(), None).await?;
        let port = port_of(running.base_url());
        Ok(Self {
            port,
            running: Mutex::new(Some(running)),
        })
    }

    /// The port, the same for the rig's whole life.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Whether it is answering now.
    pub fn is_running(&self) -> bool {
        self.running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }

    /// How many chat requests it has had (across stops).
    pub fn chats(&self) -> usize {
        self.running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map_or(0, |running| running.chats().len())
    }

    /// Stops it and returns once the port refuses connections (the serving task ends a moment
    /// after it is dropped), so a request sent next sees a fake that is down. `false` when it
    /// was already stopped.
    pub async fn stop(&self) -> bool {
        let was_running = self
            .running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
            .is_some();
        if was_running {
            let deadline = porter_fake::Deadline::generous();
            while !deadline.passed() {
                if tokio::net::TcpStream::connect(("127.0.0.1", self.port))
                    .await
                    .is_err()
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }
        was_running
    }

    /// Starts it again on the same port. `false` when it was running.
    pub async fn start(&self) -> io::Result<bool> {
        if self.is_running() {
            return Ok(false);
        }
        let fake = FakeModels::bind_on(&Bind::Port(self.port), Wire::Ollama, ollama_models(), None)
            .await?;
        let handle = fake.handle();
        *self.running.lock().unwrap_or_else(PoisonError::into_inner) =
            Some(Running::spawn(fake, handle));
        Ok(true)
    }
}

/// The port of `http://127.0.0.1:<port>`.
fn port_of(url: &str) -> u16 {
    url.rsplit(':')
        .next()
        .and_then(|p| p.trim_end_matches('/').parse().ok())
        .unwrap_or(0)
}

fn http_at(url: &str, user: Option<&str>, password: Option<&str>) -> HttpAt {
    HttpAt {
        url: url.to_owned(),
        port: port_of(url),
        user: user.map(str::to_owned),
        password: password.map(str::to_owned),
    }
}

fn mail_at(handle: &MailHandle, tls: Tls) -> MailAt {
    let port = match handle.address() {
        porter_fake::FakeAddress::Loopback(port) => *port,
        porter_fake::FakeAddress::Socket(_) => 0,
    };
    MailAt {
        host: "127.0.0.1".to_owned(),
        port,
        tls: tls_word(tls).to_owned(),
        user: MAIL_USER.to_owned(),
        password: MAIL_PASSWORD.to_owned(),
    }
}

/// The fakes that are running. Dropping it stops them all.
#[derive(Debug, Default)]
pub struct Rig {
    imap: Option<Running<MailHandle>>,
    smtp: Option<Running<MailHandle>>,
    pop3: Option<Running<MailHandle>>,
    dav: Option<Running<DavHandle>>,
    nextcloud: Option<Running<NextcloudHandle>>,
    issuer: Option<Running<IssuerHandle>>,
    graph: Option<Running<GraphHandle>>,
    google: Option<Google>,
    llm_api: Option<Running<LlmApiHandle>>,
    ollama: Option<Arc<Ollama>>,
    tls: Option<Tls>,
}

impl Rig {
    /// Starts what `options` asks for and writes the scratch CA into `options.dir`.
    pub async fn start(options: &Options) -> io::Result<Self> {
        std::fs::create_dir_all(&options.dir)?;
        std::fs::write(options.dir.join("ca.pem"), tls::CA_PEM)?;
        let accounts = || Accounts::password(MAIL_USER, MAIL_PASSWORD);
        let mut rig = Rig {
            tls: Some(options.tls),
            ..Rig::default()
        };
        if options.imap {
            rig.imap =
                Some(FakeImap::start(&Bind::Loopback, options.tls, accounts(), mailbox(3)).await?);
        }
        if options.smtp {
            rig.smtp = Some(FakeSmtp::start(&Bind::Loopback, options.tls, accounts()).await?);
        }
        if options.pop3 {
            rig.pop3 =
                Some(FakePop3::start(&Bind::Loopback, options.tls, accounts(), mailbox(3)).await?);
        }
        if options.dav {
            rig.dav = Some(FakeDav::start(DAV_USER, DAV_PASSWORD).await?);
        }
        if options.nextcloud {
            let nextcloud = FakeNextcloud::start(DAV_USER).await?;
            nextcloud.seed_app_password(NEXTCLOUD_APP_PASSWORD);
            rig.nextcloud = Some(nextcloud);
        }
        if options.oauth || options.graph {
            let issuer = FakeIssuer::start().await?;
            issuer.seed_refresh_as(OAUTH_REFRESH_TOKEN, OAUTH_CLIENT, OAUTH_SCOPE);
            issuer.seed_access_as(OAUTH_ACCESS_TOKEN, OAUTH_CLIENT);
            if let Some(seconds) = options.token_lifetime_s {
                issuer.set_token_lifetime(seconds);
            }
            rig.issuer = Some(issuer);
        }
        if let (true, Some(issuer)) = (options.graph, &rig.issuer) {
            let graph = FakeGraph::start_issued(issuer).await?;
            // The account the Microsoft sign-in reads from `GET /v1.0/me` is the rig's one user.
            graph.set_mail(MAIL_USER);
            rig.graph = Some(graph);
        }
        if options.google {
            let google = Google::start(GOOGLE_CLIENT_SECRET).await?;
            if let Some(seconds) = options.token_lifetime_s {
                google.issuer.set_token_lifetime(seconds);
            }
            rig.google = Some(google);
        }
        if options.llm_api {
            let api = FakeLlmApi::start(Auth::Bearer).await?;
            api.seed_key(API_KEY);
            rig.llm_api = Some(api);
        }
        if options.ollama {
            rig.ollama = Some(Arc::new(Ollama::first().await?));
        }
        Ok(rig)
    }

    /// The issuer's handle, when it runs.
    pub fn issuer(&self) -> Option<IssuerHandle> {
        self.issuer.as_ref().map(|running| (**running).clone())
    }

    /// The IMAP server's handle, when it runs.
    pub fn imap(&self) -> Option<MailHandle> {
        self.imap.as_ref().map(|running| (**running).clone())
    }

    /// The SMTP server's handle, when it runs.
    pub fn smtp(&self) -> Option<MailHandle> {
        self.smtp.as_ref().map(|running| (**running).clone())
    }

    /// The POP3 server's handle, when it runs.
    pub fn pop3(&self) -> Option<MailHandle> {
        self.pop3.as_ref().map(|running| (**running).clone())
    }

    /// The fake Google's issuer and APIs, when they run.
    pub fn google(&self) -> Option<&Google> {
        self.google.as_ref()
    }

    /// The Graph drive's handle, when it runs.
    pub fn graph(&self) -> Option<GraphHandle> {
        self.graph.as_ref().map(|running| (**running).clone())
    }

    /// The fake Ollama, when it runs.
    pub fn ollama(&self) -> Option<Arc<Ollama>> {
        self.ollama.clone()
    }

    /// What `rig.json` says of this rig, for the scratch directory `dir` and the control
    /// endpoint at `control`.
    pub fn describe(&self, dir: &Path, control: &str) -> RigFile {
        let tls = self.tls.unwrap_or(Tls::Implicit);
        RigFile {
            dir: dir.to_owned(),
            pid: std::process::id(),
            ca: dir.join("ca.pem"),
            control: control.to_owned(),
            secrets: crate::planted::ALL
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
            imap: self.imap.as_ref().map(|r| mail_at(r, tls)),
            smtp: self.smtp.as_ref().map(|r| mail_at(r, tls)),
            pop3: self.pop3.as_ref().map(|r| mail_at(r, tls)),
            dav: self
                .dav
                .as_ref()
                .map(|r| http_at(r.base_url(), Some(DAV_USER), Some(DAV_PASSWORD))),
            nextcloud: self
                .nextcloud
                .as_ref()
                .map(|r| http_at(r.base_url(), Some(DAV_USER), Some(NEXTCLOUD_APP_PASSWORD))),
            oauth: self.issuer.as_ref().map(|r| {
                let url = r.base_url().to_owned();
                OauthAt {
                    port: port_of(&url),
                    authorize: format!("{url}/authorize"),
                    token: format!("{url}/token"),
                    revoke: format!("{url}/revoke"),
                    device: format!("{url}/device"),
                    url,
                    client_id: OAUTH_CLIENT.to_owned(),
                    scope: OAUTH_SCOPE.to_owned(),
                    refresh_token: OAUTH_REFRESH_TOKEN.to_owned(),
                    access_token: OAUTH_ACCESS_TOKEN.to_owned(),
                }
            }),
            graph: self
                .graph
                .as_ref()
                .map(|r| http_at(r.base_url(), None, None)),
            google: self.google.as_ref().map(|g| {
                let url = g.issuer.base_url().to_owned();
                GoogleAt {
                    port: port_of(&url),
                    authorize: format!("{url}/authorize"),
                    token: format!("{url}/token"),
                    revoke: format!("{url}/revoke"),
                    url,
                    api: g.api.base_url().to_owned(),
                    api_port: port_of(g.api.base_url()),
                    userinfo: g.api.userinfo_url(),
                    client_id: GOOGLE_CLIENT.to_owned(),
                    client_secret: GOOGLE_CLIENT_SECRET.to_owned(),
                }
            }),
            ollama: self.ollama.as_ref().map(|o| OllamaAt {
                url: format!("http://127.0.0.1:{}", o.port()),
                port: o.port(),
                models: ollama_models().iter().map(|m| m.name.clone()).collect(),
            }),
            llm_api: self.llm_api.as_ref().map(|r| LlmApiAt {
                url: r.base_url().to_owned(),
                port: port_of(r.base_url()),
                api_url: r.api_url(),
                auth: "bearer".to_owned(),
                api_key: API_KEY.to_owned(),
            }),
        }
    }
}
