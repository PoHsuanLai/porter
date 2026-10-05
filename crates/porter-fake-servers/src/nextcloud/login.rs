//! Login Flow v2: `POST /index.php/login/v2` starts a flow, the person opens its `login` URL,
//! and the client polls until the app password is ready.

use crate::http::{Request, Response};
use std::collections::HashMap;

/// What the person does when they open the login page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LoginPolicy {
    /// Nothing until the test approves a flow by token.
    #[default]
    Pending,
    /// Approves at once (the scripted browser).
    Approve,
    /// Never approves: the poll keeps answering 404, as Nextcloud does for a refused login.
    Deny,
}

#[derive(Debug, PartialEq, Eq)]
enum Flow {
    Waiting,
    Ready(String),
}

/// The open flows.
#[derive(Debug, Default)]
pub struct Logins {
    pub policy: LoginPolicy,
    counter: u64,
    flows: HashMap<String, Flow>,
}

impl Logins {
    /// Approves the flow with `token`, minting its app password.
    pub fn approve(&mut self, token: &str, app_passwords: &mut Vec<String>) {
        self.counter += 1;
        let password = format!("fake-app-password-{}", self.counter);
        if let Some(flow @ Flow::Waiting) = self.flows.get_mut(token) {
            app_passwords.push(password.clone());
            *flow = Flow::Ready(password);
        }
    }

    pub fn route(
        &mut self,
        request: &Request,
        base: &str,
        user: &str,
        app_passwords: &mut Vec<String>,
    ) -> Response {
        let path = request.path();
        match (request.method.as_str(), path) {
            ("POST", "/index.php/login/v2") => {
                self.counter += 1;
                let token = format!("fake-flow-token-{}", self.counter);
                self.flows.insert(token.clone(), Flow::Waiting);
                Response::json(
                    200,
                    &serde_json::json!({
                        "poll": { "token": token, "endpoint": format!("{base}/index.php/login/v2/poll") },
                        "login": format!("{base}/index.php/login/v2/flow/{token}"),
                    }),
                )
            }
            ("GET", p) if p.starts_with("/index.php/login/v2/flow/") => {
                let token = p.trim_start_matches("/index.php/login/v2/flow/").to_owned();
                match (self.flows.contains_key(&token), self.policy) {
                    (false, _) => Response::new(404),
                    (true, policy) => {
                        if policy == LoginPolicy::Approve {
                            self.approve(&token, app_passwords);
                        }
                        Response::new(200).typed(
                            "text/html",
                            "<html><body>Grant access to the fake client</body></html>",
                        )
                    }
                }
            }
            ("POST", "/index.php/login/v2/poll") => {
                let token = request.form_value("token").unwrap_or_default();
                match self.flows.get(&token) {
                    Some(Flow::Ready(password)) => {
                        let body = serde_json::json!({ "server": base, "loginName": user, "appPassword": password });
                        self.flows.remove(&token);
                        Response::json(200, &body)
                    }
                    _ => Response::new(404),
                }
            }
            _ => Response::new(404),
        }
    }
}
