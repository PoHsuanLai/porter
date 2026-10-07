use super::*;
use crate::testkit::Scratch;
use std::os::unix::net::UnixListener;

fn tail_of(chunks: &[&[u8]]) -> Tail {
    let mut buf = TailBuf::default();
    for chunk in chunks {
        buf.push(chunk);
    }
    buf.finish();
    buf.tail()
}

#[test]
fn lines_are_cleaned_and_split_wherever_the_chunks_break() {
    type Case = (&'static str, Vec<&'static [u8]>, Vec<&'static str>);
    let cases: [Case; 6] = [
        ("one line", vec![b"boom\n"], vec!["boom"]),
        (
            "a line across chunks",
            vec![b"Trit", b"on faile", b"d\nnext\n"],
            vec!["Triton failed", "next"],
        ),
        (
            "no final break",
            vec![b"a\nlast words"],
            vec!["a", "last words"],
        ),
        (
            "control characters and colours go",
            vec![b"\x1b[31mred\x1b[0m\ttab\x07 bell\r\n"],
            vec!["red tab bell"],
        ),
        (
            "blank lines are not kept",
            vec![b"\n\n  \nx\n\n"],
            vec!["x"],
        ),
        (
            "bytes that are not text are replaced",
            vec![b"bad \xff\xfe byte\n"],
            vec!["bad \u{fffd}\u{fffd} byte"],
        ),
    ];
    for (name, chunks, expected) in cases {
        assert_eq!(tail_of(&chunks).0, expected, "{name}");
    }
}

#[test]
fn only_the_last_forty_lines_are_kept() {
    let text: String = (0..100).map(|n| format!("line {n}\n")).collect();
    let tail = tail_of(&[text.as_bytes()]);
    assert_eq!(tail.0.len(), TAIL_LINES);
    assert_eq!(tail.0.first().map(String::as_str), Some("line 60"));
    assert_eq!(tail.0.last().map(String::as_str), Some("line 99"));
}

#[test]
fn the_tail_never_holds_more_than_eight_kib() {
    // Forty lines of 500 bytes would be 20 000 bytes.
    let line = format!("{}\n", "x".repeat(500));
    let text = line.repeat(60);
    let tail = tail_of(&[text.as_bytes()]);
    let bytes: usize = tail.0.iter().map(String::len).sum();
    assert!(bytes <= TAIL_BYTES, "{bytes}");
    assert!(tail.0.len() < TAIL_LINES);
    // A line that never ends is cut at the line limit, not buffered whole.
    let endless = vec![b'y'; 100_000];
    let tail = tail_of(&[&endless]);
    assert_eq!(tail.0.len(), 1);
    assert_eq!(tail.0[0].len(), LINE_BYTES);
}

#[test]
fn the_short_tail_is_the_last_line_cut_at_a_character() {
    assert_eq!(Tail::default().short(), None);
    let tail = Tail(vec!["first".into(), "second".into()]);
    assert_eq!(tail.short().as_deref(), Some("second"));
    let long = Tail(vec!["é".repeat(300)]);
    let short = long.short().expect("a line");
    assert_eq!(short.len(), 200);
    assert!(short.chars().all(|c| c == 'é'));
}

#[test]
fn a_cause_reads_as_its_exit_status_and_last_line() {
    let cases = [
        (
            Cause::Exited {
                code: ExitCode(3),
                tail: Tail(vec!["a".into(), "Triton compile failed".into()]),
            },
            "the engine exited with status 3 before it was ready: Triton compile failed",
        ),
        (
            Cause::Exited {
                code: ExitCode(-1),
                tail: Tail::default(),
            },
            "the engine was killed before it was ready",
        ),
        (
            Cause::NeverReady {
                tail: Tail(vec!["loading".into()]),
            },
            "the engine did not become ready in time: loading",
        ),
    ];
    for (cause, expected) in cases {
        assert_eq!(cause.to_string(), expected);
    }
    let long = Cause::SocketPathTooLong {
        path: "/a/b".into(),
        len: 130,
    }
    .to_string();
    assert!(
        long.contains("/a/b") && long.contains("130") && long.contains("108"),
        "{long}"
    );
}

#[test]
fn a_socket_path_of_108_bytes_is_refused_and_107_is_not() {
    let scratch = Scratch::new("startup-len");
    let base = scratch.path().to_string_lossy().len();
    // The path is the scratch directory, a slash and a name: 107 and 108 bytes in all.
    let name = |total: usize| "n".repeat(total - base - 1);
    let fits = scratch.path().join(name(107));
    let too_long = scratch.path().join(name(108));
    assert_eq!(fits.as_os_str().len(), 107);
    assert_eq!(clear_socket(&fits), Ok(()));
    assert_eq!(
        clear_socket(&too_long),
        Err(Cause::SocketPathTooLong {
            path: too_long.clone(),
            len: 108
        })
    );
    // The bytes count, not the characters.
    let wide = scratch.path().join("é".repeat((108 - base) / 2));
    assert!(wide.as_os_str().len() >= SUN_PATH);
    assert!(matches!(
        clear_socket(&wide),
        Err(Cause::SocketPathTooLong { .. })
    ));
}

#[test]
fn a_stale_socket_is_removed_and_anything_else_is_kept() {
    let scratch = Scratch::new("startup-clear");
    let dir = scratch.path();

    let absent = dir.join("absent.sock");
    assert_eq!(clear_socket(&absent), Ok(()));

    // A socket an earlier run left behind: bound, then the owner died (the file stays).
    let stale = dir.join("stale.sock");
    drop(UnixListener::bind(&stale).expect("bind"));
    assert!(stale.exists());
    assert_eq!(clear_socket(&stale), Ok(()));
    assert!(!stale.exists());

    // A regular file is refused and kept, with its contents.
    let file = dir.join("file.sock");
    std::fs::write(&file, b"mine").expect("write");
    assert_eq!(
        clear_socket(&file),
        Err(Cause::SocketPathTaken { path: file.clone() })
    );
    assert_eq!(std::fs::read(&file).expect("kept"), b"mine");

    // So is a directory, and a symlink (even one that points at a socket).
    let folder = dir.join("folder.sock");
    std::fs::create_dir(&folder).expect("dir");
    assert!(matches!(
        clear_socket(&folder),
        Err(Cause::SocketPathTaken { .. })
    ));
    let target = dir.join("target.sock");
    let _live = UnixListener::bind(&target).expect("bind");
    let link = dir.join("link.sock");
    std::os::unix::fs::symlink(&target, &link).expect("symlink");
    assert!(matches!(
        clear_socket(&link),
        Err(Cause::SocketPathTaken { .. })
    ));
    assert!(target.exists(), "the socket behind a link is not touched");
}

#[test]
fn diagnostics_forget_the_last_start_when_a_new_one_begins() {
    let diagnostics = Diagnostics::default();
    let id = EngineId("llama-server:x".into());
    assert_eq!(diagnostics.refusal(&id), None);
    assert_eq!(diagnostics.tail(&id), Tail::default());
    let live = diagnostics.begin(&id);
    live.lock().expect("lock").push(b"said this\nand half");
    diagnostics.refuse(&id, Cause::Refused);
    assert_eq!(diagnostics.refusal(&id), Some(Cause::Refused));
    // The line the engine has not ended yet is part of what it said.
    assert_eq!(
        diagnostics.tail(&id).0,
        vec!["said this".to_owned(), "and half".to_owned()]
    );
    diagnostics.begin(&id);
    assert_eq!(diagnostics.refusal(&id), None);
    assert_eq!(diagnostics.tail(&id), Tail::default());
}

#[test]
fn a_failed_start_logs_at_the_level_its_cause_calls_for() {
    let lines = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&lines);
    let log = Log::to(move |level, text| sink.lock().expect("lock").push((level, text.to_owned())));
    let id = EngineId("vllm:m".into());
    log.failed(
        &id,
        &Cause::Exited {
            code: ExitCode(3),
            tail: Tail(vec!["first".into(), "second".into()]),
        },
    );
    log.failed(
        &id,
        &Cause::SocketPathTooLong {
            path: "/p".into(),
            len: 200,
        },
    );
    let lines = lines.lock().expect("lock");
    assert_eq!(lines[0].0, Level::Warn);
    assert!(lines[0].1.contains("status 3"), "{}", lines[0].1);
    assert!(
        lines[0].1.contains("| first") && lines[0].1.contains("| second"),
        "{}",
        lines[0].1
    );
    assert_eq!(lines[1].0, Level::Error);
    assert!(
        lines[1].1.contains("/p") && lines[1].1.contains("200"),
        "{}",
        lines[1].1
    );
    assert!(!lines[1].1.contains("standard error"));
}
