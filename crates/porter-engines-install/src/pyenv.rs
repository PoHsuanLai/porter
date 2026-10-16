//! A Python environment built from a hashed lock: the pinned `uv` and the pinned Python are
//! unpacked into the install, and `uv` fills a virtual environment from the lock with every
//! hash required, no resolving and no building from source. The environment is made where it
//! stays (its scripts name their own path), so the install is finished by writing its record
//! last: a directory with no record is not an install.

use crate::extract::{settle_modes, unpack};
use crate::fetch::Fetcher;
use crate::installer::{InstallError, Installer, blocking, record_of};
use crate::progress::{CancelToken, Meter};
use porter_engines::{Delivery, InstallDir, Row, entry_digests, seal_of};
use std::ffi::OsString;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

/// How often the bytes `uv` has fetched are counted.
const TICK: Duration = Duration::from_secs(1);

/// Builds the environment of `row` in `dir`.
pub(crate) async fn build(
    fetcher: &Fetcher,
    dir: &InstallDir,
    row: &'static Row,
    meter: &Meter,
    cancel: &CancelToken,
) -> Result<(), InstallError> {
    let Delivery::Python(env) = row.delivery else {
        return Err(InstallError::Environment);
    };
    let files = Installer::download(fetcher, dir, row, meter, cancel).await?;
    let (tree, partial) = (dir.tree(), dir.partial());
    let (at, work) = (dir.path().to_path_buf(), partial.clone());
    blocking(move || {
        let _old = std::fs::remove_dir_all(&at);
        let (uv, python) = (at.join("engine/uv"), at.join("engine/python"));
        for made in [&uv, &python] {
            std::fs::create_dir_all(made).map_err(|e| InstallError::Disk(e.kind()))?;
        }
        let [uv_archive, python_archive] = &files[..] else {
            return Err(InstallError::Environment);
        };
        unpack(uv_archive, &uv)?;
        unpack(python_archive, &python)?;
        settle_modes(&uv)?;
        settle_modes(&python)?;
        if !seal_of(&python)?.is(env.python_seal) {
            return Err(InstallError::SealMismatch);
        }
        std::fs::write(work.join("lock.txt"), env.lock).map_err(|e| InstallError::Disk(e.kind()))
    })
    .await?;
    let (uv, python, venv) = (
        tree.join("uv/uv"),
        tree.join("python/bin/python3"),
        tree.join("venv"),
    );
    let floor = meter.done();
    run(
        &uv,
        vec![
            "venv".into(),
            "--no-config".into(),
            "--no-python-downloads".into(),
            "--python".into(),
            python.into(),
            venv.clone().into(),
        ],
        &partial,
        (meter, floor),
        cancel,
    )
    .await?;
    run(
        &uv,
        vec![
            "pip".into(),
            "install".into(),
            "--no-config".into(),
            "--python".into(),
            venv.join("bin/python").into(),
            "--require-hashes".into(),
            "--no-deps".into(),
            "--no-build".into(),
            "--link-mode".into(),
            "copy".into(),
            "--compile-bytecode".into(),
            "-r".into(),
            partial.join("lock.txt").into(),
        ],
        &partial,
        (meter, floor),
        cancel,
    )
    .await?;
    let (tree_done, record_at, package, pinned) =
        (tree.clone(), dir.record(), env.package, env.python_seal);
    let mut record = record_of(row);
    blocking(move || {
        record.seal = pinned.to_owned();
        record.entries = entry_digests(&tree_done, package)?;
        record.venv_seal = Some(seal_of(&tree_done.join("venv"))?.as_hex().to_owned());
        record
            .write(&record_at)
            .map_err(|_| InstallError::Disk(std::io::ErrorKind::Other))
    })
    .await
}

/// What ended a wait on the child.
enum Ended {
    Exited(std::io::Result<std::process::ExitStatus>),
    Stopped,
    Tick,
}

/// Runs `uv` with a scrubbed environment of its own: nothing it reads or writes is outside
/// `work`. The meter follows the bytes it has put in its cache.
async fn run(
    program: &Path,
    args: Vec<OsString>,
    work: &Path,
    (meter, floor): (&Meter, u64),
    cancel: &CancelToken,
) -> Result<(), InstallError> {
    let (cache, temp) = (work.join("cache"), work.join("tmp"));
    std::fs::create_dir_all(&temp).map_err(|e| InstallError::Disk(e.kind()))?;
    let mut child = Command::new(program)
        .args(args)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", work)
        .env("TMPDIR", &temp)
        .env("UV_CACHE_DIR", &cache)
        .env("UV_PYTHON_DOWNLOADS", "never")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| InstallError::Environment)?;
    let mut tick = tokio::time::interval(TICK);
    loop {
        let ended = tokio::select! {
            status = child.wait() => Ended::Exited(status),
            () = cancel.cancelled() => Ended::Stopped,
            _ = tick.tick() => Ended::Tick,
        };
        match ended {
            Ended::Exited(Ok(status)) if status.success() => return Ok(()),
            Ended::Exited(_) => return Err(InstallError::Environment),
            Ended::Stopped => {
                let _killed = child.kill().await;
                return Err(InstallError::Cancelled);
            }
            Ended::Tick => meter.set_done(floor + dir_bytes(&cache)),
        }
    }
}

/// The bytes of the files under `root`.
fn dir_bytes(root: &Path) -> u64 {
    let mut total = 0u64;
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            match entry.metadata() {
                Ok(meta) if meta.is_dir() => pending.push(entry.path()),
                Ok(meta) => total += meta.len(),
                Err(_) => {}
            }
        }
    }
    total
}
