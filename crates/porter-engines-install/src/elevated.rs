//! Installing for everyone: the privileged helper, started through `pkexec`.
//!
//! The caller gives `pkexec` the helper's fixed path and two words, an engine and a backend, and
//! reads the helper's report lines. It asks for nothing else and takes nothing else from it: a
//! download's address, hash and size are in the helper's own manifest. The administrator's
//! password is asked for by the system's prompt, never here.

use crate::helper::Report;
use crate::progress::{CancelToken, Meter};
use porter_engines::{Backend, EngineId, Failure};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

/// How long a helper that was asked to stop has to do it.
const STOP_GRACE: Duration = Duration::from_secs(10);

/// What `pkexec` says when the person did not authorise: dismissed the prompt (126) or could
/// not be authorised (127).
const NOT_AUTHORISED: [i32; 2] = [126, 127];

/// The program that starts the helper and the helper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelperProgram {
    /// `pkexec`.
    pub pkexec: PathBuf,
    /// The helper.
    pub program: PathBuf,
}

impl HelperProgram {
    /// Where the system keeps them (the helper beside the daemons, in `libexec/quire`).
    pub fn system() -> Self {
        Self {
            pkexec: PathBuf::from("/usr/bin/pkexec"),
            program: PathBuf::from("/usr/libexec/quire/porter-engine-helper"),
        }
    }

    /// Whether both are there: without them nothing can be installed for everyone.
    pub fn present(&self) -> bool {
        Path::new(&self.pkexec).is_file() && Path::new(&self.program).is_file()
    }
}

/// What the wait on the helper ended in.
enum Event {
    Line(std::io::Result<Option<String>>),
    Stop,
}

/// Runs the helper for `engine` and `backend` to its end. `meter` is out of 1000 and follows the
/// helper's progress; `cancel` asks it to stop.
pub async fn install_for_everyone(
    helper: &HelperProgram,
    engine: EngineId,
    backend: Backend,
    meter: &Meter,
    cancel: &CancelToken,
) -> Result<(), Failure> {
    let mut child = Command::new(&helper.pkexec)
        .arg(&helper.program)
        .arg("install")
        .arg(engine.slug())
        .arg(backend.slug())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| Failure::Other)?;
    let (Some(stdout), mut stdin) = (child.stdout.take(), child.stdin.take()) else {
        return Err(Failure::Other);
    };
    let mut lines = BufReader::new(stdout).lines();
    let mut last = None;
    loop {
        let event = tokio::select! {
            line = lines.next_line() => Event::Line(line),
            () = cancel.cancelled() => Event::Stop,
        };
        match event {
            Event::Line(Ok(Some(line))) => match Report::parse(&line) {
                Some(Report::Progress(thousandths)) => meter.set_done(u64::from(thousandths)),
                Some(report) => last = Some(report),
                None => {}
            },
            Event::Line(_) => break,
            Event::Stop => {
                if let Some(stdin) = stdin.as_mut() {
                    // Closing its input stops it as well; the word is for a helper that is
                    // reading lines.
                    let _told = stdin.write_all(b"cancel\n").await;
                }
                drop(stdin);
                let _drained = tokio::time::timeout(STOP_GRACE, async {
                    while let Ok(Some(_)) = lines.next_line().await {}
                })
                .await;
                let _killed = child.kill().await;
                return Err(Failure::Cancelled);
            }
        }
    }
    let status = child.wait().await.map_err(|_| Failure::Other)?;
    match last {
        Some(Report::Done) => Ok(()),
        Some(Report::Failed(why)) => Err(why),
        _ => match status.code() {
            Some(code) if NOT_AUTHORISED.contains(&code) => Err(Failure::Declined),
            _ => Err(Failure::Other),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_fake::Scratch;

    /// A helper that says what the script says, with `/bin/sh` standing for pkexec: the script's
    /// first line exits with `pkexec_code` when that is not 0, as pkexec does when the person did
    /// not authorise. The script is only read by the shell, never executed itself: a file just
    /// written can be held open for writing by a child a parallel test is forking, and executing
    /// it then fails with "text file busy".
    fn fake(dir: &Scratch, pkexec_code: i32, helper_says: &str) -> HelperProgram {
        let program = dir.path().join("helper");
        std::fs::write(
            &program,
            format!("[ {pkexec_code} -ne 0 ] && exit {pkexec_code}\n{helper_says}\n"),
        )
        .expect("script");
        HelperProgram {
            pkexec: PathBuf::from("/bin/sh"),
            program,
        }
    }

    async fn run(helper: &HelperProgram) -> (Result<(), Failure>, Meter) {
        let meter = Meter::new(1000);
        let result = install_for_everyone(
            helper,
            EngineId::LlamaCpp,
            Backend::Cpu,
            &meter,
            &CancelToken::new(),
        )
        .await;
        (result, meter)
    }

    #[tokio::test]
    async fn the_helpers_report_lines_are_followed_to_done_or_to_the_reason_it_gave() {
        let dir = Scratch::new("system-reports");
        let done = fake(&dir, 0, "echo progress 400\necho progress 1000\necho done");
        let (result, meter) = run(&done).await;
        assert_eq!((result, meter.permille()), (Ok(()), 1000));
        let failed = fake(&dir, 0, "echo progress 10\necho failed no_space\nexit 1");
        assert_eq!(run(&failed).await.0, Err(Failure::NoSpace));
        let silent = fake(&dir, 0, "exit 1");
        assert_eq!(run(&silent).await.0, Err(Failure::Other));
    }

    #[tokio::test]
    async fn a_prompt_the_person_dismissed_or_could_not_pass_is_declined() {
        let dir = Scratch::new("system-declined");
        for code in [126, 127] {
            let helper = fake(&dir, code, "echo done");
            assert_eq!(run(&helper).await.0, Err(Failure::Declined), "{code}");
        }
    }

    #[tokio::test]
    async fn a_stop_tells_the_helper_and_ends_the_job_cancelled() {
        let dir = Scratch::new("system-stop");
        let slow = fake(&dir, 0, "echo progress 1\nread line\necho failed cancelled");
        let cancel = CancelToken::new();
        let meter = Meter::new(1000);
        let job = {
            let (cancel, slow, meter) = (cancel.clone(), slow.clone(), meter.clone());
            tokio::spawn(async move {
                install_for_everyone(&slow, EngineId::LlamaCpp, Backend::Cpu, &meter, &cancel).await
            })
        };
        tokio::time::timeout(Duration::from_secs(10), async {
            while meter.done() == 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the helper reports its progress within 10 s");
        cancel.cancel();
        assert_eq!(job.await.expect("joined"), Err(Failure::Cancelled));
    }

    #[test]
    fn the_helper_is_present_only_when_both_programs_are() {
        let dir = Scratch::new("system-present");
        let mut helper = fake(&dir, 0, "echo done");
        assert!(helper.present());
        helper.program = dir.path().join("gone");
        assert!(!helper.present());
    }
}
