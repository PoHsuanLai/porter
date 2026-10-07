use super::*;
use crate::testkit::Scratch;
use model_http::Secret;
use std::os::unix::fs::PermissionsExt;

fn file_with(scratch: &Scratch, text: &[u8], mode: u32) -> KeyFile {
    let path = scratch.path().join("key");
    std::fs::write(&path, text).expect("write");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).expect("chmod");
    KeyFile::at(path)
}

#[test]
fn a_private_file_is_the_token_without_its_line_break() {
    let scratch = Scratch::new("key");
    let key = file_with(&scratch, b"sk-lab-1234\n", 0o600);
    assert_eq!(key.read(), Ok(Secret("sk-lab-1234".into())));
    // Read at each connect: a rotated token is the one the next read gives.
    let key = file_with(&scratch, b"sk-lab-5678", 0o600);
    assert_eq!(key.read(), Ok(Secret("sk-lab-5678".into())));
}

#[test]
fn a_file_others_can_read_is_refused_and_so_is_one_that_is_not_a_file() {
    let scratch = Scratch::new("key");
    for mode in [0o644, 0o640, 0o604, 0o660, 0o666, 0o700 | 0o010] {
        let key = file_with(&scratch, b"sk-lab-1234", mode);
        assert_eq!(
            key.read(),
            Err(KeyFileProblem::NotPrivate { mode }),
            "{mode:04o}"
        );
    }
    // Nothing of the token is in what is said.
    let said = format!("{:?}", file_with(&scratch, b"sk-lab-1234", 0o644).read());
    assert!(!said.contains("sk-lab"));
    let dir = KeyFile::at(scratch.path().to_path_buf());
    assert!(matches!(
        dir.read(),
        Err(KeyFileProblem::NotAFile | KeyFileProblem::NotPrivate { .. })
    ));
    assert_eq!(
        KeyFile::at(scratch.path().join("nothing")).read(),
        Err(KeyFileProblem::Missing)
    );
}

#[test]
fn an_empty_or_odd_token_is_not_used() {
    let scratch = Scratch::new("key");
    for text in [
        &b""[..],
        b"  \n",
        b"two words",
        b"tab\there",
        b"\xff\xfe",
        &[b'a'; 5000],
    ] {
        assert_eq!(
            file_with(&scratch, text, 0o600).read(),
            Err(KeyFileProblem::Unusable),
            "{text:?}"
        );
    }
}
