//! A key handed to a process the launcher spawns (agent-session ask P2, lane p2-handoff): what
//! `Launcher::issue_credential` returns, how the launcher sets a child up with it, and the
//! signal that tells the launcher accountd ended it.
//!
//! **The launcher's duties** (accountd cannot do these for it):
//!
//! - Give the child the key only as [`ChildKey`] says, after `env_clear`: never in the
//!   environment of the launcher itself, a log line, a command line or a file of its own.
//! - Watch [`Revocations`]. When accountd ends a credential (the grant was revoked, the account
//!   removed) it unlinks a tmpfs file, but it cannot reach the child's copy of a memfd or the
//!   text it already read, so it tells the launcher in a signal and the launcher must end the
//!   process: ask it to quit, and kill it after a grace period the launcher chooses.
//! - Call `Launcher::revoke_credential` when the process exits or is killed, so the credential
//!   does not outlive it. If the launcher's connection leaves the bus accountd ends all of its
//!   credentials, but then nobody is left to kill the children.
//! - Hand a credential to one child only, and only to the program it was issued for.
//!
//! **Residual risk**, which no choice here removes: the child holds its own key. It, and anything
//! it spawns, can read it and send it wherever its network allows. A handed-off key is outside
//! porter's meter (`ai.spend.*` caps count only what passes inferd), and outside the data-class
//! floors inferd applies to a turn. The sandbox's network rules are the mitigation; the metered,
//! key-less route is inferd as the agent's model endpoint (P4) for any agent that takes a base
//! URL. A memfd keeps the key out of `/proc/<pid>/environ`; delivering it as text in the child's
//! environment ([`Delivery::Value`]) puts it back there, readable by processes of the same user.

use crate::launcher::LauncherError;
use porter_core::audit::{CredentialEnd, Handoff};
use porter_core::capability::EnvName;
use porter_core::{ProcessCredentialId, SecretText};
use porter_dbus::{BusStream, ProcessCredentialRevoked, ProcessCredentialRevokedStream};
use std::io;
use std::os::fd::{AsFd, OwnedFd, RawFd};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::task::{Context, Poll};

/// Where the key is, for the launcher to pass on.
#[derive(Debug)]
pub enum CredentialHandle {
    /// A sealed memfd, read position at the start; the launcher passes it to the child as an
    /// inherited descriptor. Read it with `pread` or by opening `/proc/self/fd/<n>`: a plain
    /// `read` moves the position every holder of the descriptor shares.
    Memfd(OwnedFd),
    /// A 0600 file in a 0700 directory under `$XDG_RUNTIME_DIR`, which accountd unlinks when the
    /// credential ends.
    TmpfsFile(PathBuf),
}

/// A credential issued to this launcher for one process.
#[derive(Debug)]
pub struct ProcessCredential {
    id: ProcessCredentialId,
    handle: CredentialHandle,
}

/// How the child is told where its key is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// The variable holds the key itself. For a program that reads only its key variable (most
    /// do). The key is then in the child's environment, which same-user processes can read in
    /// `/proc/<pid>/environ`: the residual risk named above.
    Value,
    /// The variable `<KEY_VAR>_FILE` holds a path to a file with the key, and the key is in no
    /// environment. Only for a program that is known to read such a variable or that is
    /// started through a wrapper that does; porter declares no program that does. A memfd is
    /// shown to the child as descriptor `child_fd` (`/proc/self/fd/<child_fd>`); a tmpfs file by
    /// its path.
    File {
        /// The descriptor number the child sees the memfd at. Ignored for a tmpfs file.
        child_fd: RawFd,
    },
}

/// What to give the child, from [`ProcessCredential::child_key`].
#[derive(Debug)]
pub enum ChildKey {
    /// Set `var` to the key text in the child's environment, after `env_clear`.
    Value {
        /// The variable (`AgentCap.key_env`).
        var: EnvName,
        /// The key.
        value: SecretText,
    },
    /// Set `var` to `path`, which names a file that holds the key.
    File {
        /// `<AgentCap.key_env>_FILE`.
        var: EnvName,
        /// `/proc/self/fd/<n>` or the tmpfs path.
        path: String,
        /// For a memfd: a duplicate to place at descriptor number `number` in the child (`dup2`
        /// in a `pre_exec` hook, or the spawner's descriptor-mapping call), and to close in the
        /// launcher once the child has started.
        inherit: Option<Inherit>,
    },
}

/// A descriptor the child must inherit, and the number it must have there.
#[derive(Debug)]
pub struct Inherit {
    /// A duplicate of the credential's memfd.
    pub fd: OwnedFd,
    /// The number the child sees it at.
    pub number: RawFd,
}

/// Why a child could not be set up.
#[derive(Debug, thiserror::Error)]
pub enum ChildKeyError {
    /// The key could not be read back from the descriptor or the file.
    #[error("the key could not be read: {0}")]
    Unreadable(String),
    /// The key is not text.
    #[error("the key is not text")]
    NotText,
    /// `<key variable>_FILE` is not a valid variable name (the variable's name is too long).
    #[error("`{0}_FILE` is not a variable name")]
    NoFileVariable(String),
}

impl ProcessCredential {
    pub(crate) fn new(id: ProcessCredentialId, handle: CredentialHandle) -> Self {
        Self { id, handle }
    }

    /// The id to end it by (`Launcher::revoke_credential`) and that `Revocations` names.
    pub fn id(&self) -> &ProcessCredentialId {
        &self.id
    }

    /// The way it was handed over.
    pub fn handoff(&self) -> Handoff {
        match self.handle {
            CredentialHandle::Memfd(_) => Handoff::Memfd,
            CredentialHandle::TmpfsFile(_) => Handoff::TmpfsFile,
        }
    }

    /// Where the key is.
    pub fn handle(&self) -> &CredentialHandle {
        &self.handle
    }

    /// The key's text, read without moving a shared position.
    pub fn read_key(&self) -> Result<SecretText, ChildKeyError> {
        let bytes = match &self.handle {
            CredentialHandle::Memfd(fd) => read_all_at(fd),
            CredentialHandle::TmpfsFile(path) => std::fs::read(path),
        }
        .map_err(|why| ChildKeyError::Unreadable(why.kind().to_string()))?;
        String::from_utf8(bytes)
            .map(SecretText::new)
            .map_err(|_| ChildKeyError::NotText)
    }

    /// What to give the child whose key variable is `key_env` (`AgentCap.key_env`), by `delivery`.
    pub fn child_key(
        &self,
        key_env: &EnvName,
        delivery: Delivery,
    ) -> Result<ChildKey, ChildKeyError> {
        match delivery {
            Delivery::Value => Ok(ChildKey::Value {
                var: key_env.clone(),
                value: self.read_key()?,
            }),
            Delivery::File { child_fd } => {
                let var = EnvName::parse(&format!("{}_FILE", key_env.as_str()))
                    .map_err(|_| ChildKeyError::NoFileVariable(key_env.as_str().to_owned()))?;
                match &self.handle {
                    CredentialHandle::Memfd(fd) => {
                        let copy = fd
                            .as_fd()
                            .try_clone_to_owned()
                            .map_err(|why| ChildKeyError::Unreadable(why.kind().to_string()))?;
                        Ok(ChildKey::File {
                            var,
                            path: format!("/proc/self/fd/{child_fd}"),
                            inherit: Some(Inherit {
                                fd: copy,
                                number: child_fd,
                            }),
                        })
                    }
                    CredentialHandle::TmpfsFile(path) => Ok(ChildKey::File {
                        var,
                        path: path_text(path),
                        inherit: None,
                    }),
                }
            }
        }
    }
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Every byte of `fd`, by `pread` from offset zero.
fn read_all_at(fd: &OwnedFd) -> io::Result<Vec<u8>> {
    let file = std::fs::File::from(fd.as_fd().try_clone_to_owned()?);
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 512];
    loop {
        let offset = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        match file.read_at(&mut chunk, offset)? {
            0 => return Ok(bytes),
            n => bytes.extend_from_slice(&chunk[..n]),
        }
    }
}

/// accountd ended a credential: the launcher must end the process that holds the key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Revoked {
    /// The credential.
    pub id: ProcessCredentialId,
    /// Why.
    pub reason: CredentialEnd,
}

/// The stream of [`Revoked`], sent to this launcher alone. An item is an error when accountd sent
/// one that is not well formed.
#[derive(Debug)]
pub struct Revocations {
    stream: ProcessCredentialRevokedStream,
}

impl Revocations {
    pub(crate) fn new(stream: ProcessCredentialRevokedStream) -> Self {
        Self { stream }
    }

    fn revoked(signal: ProcessCredentialRevoked) -> Result<Revoked, LauncherError> {
        let args = signal.args()?;
        Ok(Revoked {
            id: ProcessCredentialId::parse(&args.id)?,
            reason: CredentialEnd::from_word(&args.reason).ok_or_else(|| {
                LauncherError::Malformed(format!("`{}` is not a reason", args.reason))
            })?,
        })
    }
}

impl BusStream for Revocations {
    type Item = Result<Revoked, LauncherError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.stream)
            .poll_next(cx)
            .map(|signal| signal.map(Self::revoked))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn scratch_file(text: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("porter-client-key-{}", std::process::id()));
        let mut file = std::fs::File::create(&path).expect("create");
        file.write_all(text.as_bytes()).expect("write");
        path
    }

    fn credential(handle: CredentialHandle) -> ProcessCredential {
        ProcessCredential::new(ProcessCredentialId::parse("cred-1").expect("id"), handle)
    }

    #[test]
    fn a_file_credential_gives_its_text_or_a_path_variable_and_never_an_inherited_descriptor() {
        let path = scratch_file("sk-test-0123");
        let credential = credential(CredentialHandle::TmpfsFile(path.clone()));
        assert_eq!(credential.handoff(), Handoff::TmpfsFile);
        let var = EnvName::parse("ANTHROPIC_API_KEY").expect("var");
        match credential.child_key(&var, Delivery::Value).expect("value") {
            ChildKey::Value { var, value } => {
                assert_eq!(var.as_str(), "ANTHROPIC_API_KEY");
                assert_eq!(value.expose(), "sk-test-0123");
            }
            other => panic!("{other:?}"),
        }
        match credential
            .child_key(&var, Delivery::File { child_fd: 9 })
            .expect("file")
        {
            ChildKey::File {
                var,
                path: shown,
                inherit,
            } => {
                assert_eq!(var.as_str(), "ANTHROPIC_API_KEY_FILE");
                assert_eq!(shown, path.to_string_lossy());
                assert!(inherit.is_none());
            }
            other => panic!("{other:?}"),
        }
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_variable_too_long_to_take_a_file_suffix_is_refused() {
        let long = "K".repeat(62);
        let var = EnvName::parse(&long).expect("var");
        let credential = credential(CredentialHandle::TmpfsFile(PathBuf::from("/nowhere")));
        assert!(matches!(
            credential.child_key(&var, Delivery::File { child_fd: 3 }),
            Err(ChildKeyError::NoFileVariable(_))
        ));
    }

    #[test]
    fn a_key_that_is_gone_is_unreadable_not_empty() {
        let credential = credential(CredentialHandle::TmpfsFile(PathBuf::from(
            "/nonexistent/porter/key",
        )));
        assert!(matches!(
            credential.read_key(),
            Err(ChildKeyError::Unreadable(_))
        ));
    }
}
