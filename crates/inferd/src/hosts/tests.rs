use super::*;
use crate::testkit::Scratch;
use engine_supervisor::{EnvPair, GpuAccess, Network, ProgramPath, Sandbox};
use model_catalog::EngineArg;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixListener;

fn unit(program: &str, args: &[&str]) -> UnitSpec {
    UnitSpec {
        program: ProgramPath(PathBuf::from(program)),
        args: args.iter().map(|a| EngineArg((*a).into())).collect(),
        env: vec![EnvPair {
            name: "HF_HUB_OFFLINE".into(),
            value: "1".into(),
        }],
        sandbox: Sandbox {
            network: Network::None,
            read: vec![],
            write: vec![],
            gpu: GpuAccess::Absent,
            memory_max: MiB(0),
        },
    }
}

fn id(name: &str) -> EngineId {
    EngineId(name.into())
}

#[test]
fn the_total_is_the_first_line_of_the_csv() {
    let cases = [
        ("16303\n", Ok(MiB(16303))),
        ("16303", Ok(MiB(16303))),
        (" 12288 \n24576\n", Ok(MiB(12288))),
        ("", Err(GpuError::Unreadable)),
        ("N/A\n", Err(GpuError::Unreadable)),
        ("16303 MiB\n", Err(GpuError::Unreadable)),
    ];
    for (text, expected) in cases {
        assert_eq!(parse_total(text), expected, "{text:?}");
    }
}

#[tokio::test]
async fn nvidia_smi_that_is_not_there_is_unavailable() {
    let smi = NvidiaSmi::new(PathBuf::from("/nonexistent/nvidia-smi"));
    assert_eq!(smi.memory().await, Err(GpuError::Unavailable));
}

#[tokio::test]
async fn a_child_process_is_started_stopped_and_its_exit_seen() {
    let host = ProcessHost::new();
    host.spawn(&id("a"), &unit("sleep", &["30"]))
        .await
        .expect("spawn");
    host.stop(&id("a")).await.expect("stop");
    // A killed process has no exit code of its own.
    assert_eq!(host.exited(&id("a")).await, ExitCode(-1));
    // It is stopped once; the second stop finds nothing to stop.
    assert_eq!(host.stop(&id("a")).await, Err(HostError::NotRunning));
}

#[tokio::test]
async fn a_process_that_ends_on_its_own_reports_its_code() {
    let host = ProcessHost::new();
    host.spawn(&id("b"), &unit("sh", &["-c", "exit 3"]))
        .await
        .expect("spawn");
    assert_eq!(host.exited(&id("b")).await, ExitCode(3));
}

#[tokio::test]
async fn a_program_that_is_not_there_is_refused_and_an_unknown_engine_has_not_exited_well() {
    let host = ProcessHost::new();
    assert_eq!(
        host.spawn(&id("c"), &unit("/nonexistent/engine", &[]))
            .await,
        Err(HostError::Refused)
    );
    assert_eq!(host.exited(&id("never-started")).await, ExitCode(-1));
}

/// A socket that answers each connection with `response` and closes it.
fn answering(dir: &Scratch, name: &str, response: &'static str) -> PathBuf {
    let path = dir.path().join(name);
    let listener = UnixListener::bind(&path).expect("bind");
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut buf = [0_u8; 1024];
                let _ = stream.read(&mut buf).await;
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.shutdown().await;
            });
        }
    });
    path
}

#[tokio::test]
async fn the_health_probe_reads_the_status_off_the_engines_socket() {
    let dir = Scratch::new("hosts-health");
    let ok = answering(
        &dir,
        "ok.sock",
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}",
    );
    let loading = answering(
        &dir,
        "loading.sock",
        "HTTP/1.1 503 Service Unavailable\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}",
    );
    let probe = HealthProbe::new([
        (id("ok"), ok),
        (id("loading"), loading),
        (id("gone"), dir.path().join("nobody-listens.sock")),
    ]);
    assert_eq!(probe.probe(&id("ok")).await, Probe::Ready);
    assert_eq!(probe.probe(&id("loading")).await, Probe::Loading);
    assert_eq!(probe.probe(&id("gone")).await, Probe::Down);
    assert_eq!(probe.probe(&id("unknown")).await, Probe::Down);
}
