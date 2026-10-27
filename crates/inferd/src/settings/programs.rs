//! The rows that name the program of an engine, `engines.vllm_python` and `engines.llama_server`
//! (Advanced, in the "Local models" section): where the person says the program is when inferd
//! does not find it itself. Empty means inferd looks for it as it always did. A program is what
//! inferd runs with the person's files, so a path is taken only when nobody else could have put
//! the program there or could change it: see [`checked`].

use ds_settings::schema::{
    AgentSetting, Exposure, Help, KeyKind, KeyPath, KeySpec, Label, Page, Section,
};
use porter_daemon::held_under;
use porter_router::local::EngineConfig;
use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

/// The section the rows are listed in.
const SECTION: &str = "Local models";

/// The table of the file the rows live in.
pub(super) const TABLE: &str = "engines";

/// An engine program the person can name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Program {
    /// The Python of vLLM's environment.
    VllmPython,
    /// llama.cpp's server.
    LlamaServer,
}

impl Program {
    /// Every program with a row, in the order the rows are listed.
    pub(super) const ALL: [Program; 2] = [Program::VllmPython, Program::LlamaServer];

    /// The program a row's path names.
    pub(super) fn of_path(path: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|one| one.path() == path)
    }

    /// The key in the `engines` table.
    pub(super) fn key(self) -> &'static str {
        match self {
            Self::VllmPython => "vllm_python",
            Self::LlamaServer => "llama_server",
        }
    }

    /// The row's settings path.
    pub(super) fn path(self) -> String {
        format!("{TABLE}.{}", self.key())
    }

    fn label(self) -> &'static str {
        match self {
            Self::VllmPython => "vLLM's Python",
            Self::LlamaServer => "llama-server",
        }
    }

    fn help(self) -> &'static str {
        match self {
            Self::VllmPython => {
                "The Python program that runs vLLM. Leave it empty and this computer finds it itself."
            }
            Self::LlamaServer => {
                "The program that runs llama.cpp models. Leave it empty and this computer finds it itself."
            }
        }
    }

    /// What the file names for this program; empty when it names none.
    pub(super) fn named(self, engines: &EngineConfig) -> String {
        let program = match self {
            Self::VllmPython => engines.vllm_python.as_deref(),
            Self::LlamaServer => engines.llama_server.as_deref(),
        };
        program.map_or_else(String::new, |path| path.to_string_lossy().into_owned())
    }

    /// The row.
    pub(super) fn spec(self) -> KeySpec {
        KeySpec {
            control: None,
            path: KeyPath(self.path()),
            kind: KeyKind::Text,
            default: toml::Value::String(String::new()),
            label: Label(self.label().to_owned()),
            help: Help(self.help().to_owned()),
            page: Page::Intelligence,
            section: Section(SECTION.to_owned()),
            exposure: Exposure::Advanced,
            labels: Default::default(),
            agent: AgentSetting::HandsOff,
            unavailable: BTreeMap::new(),
            groups: BTreeMap::new(),
            places: BTreeMap::new(),
            note: None,
        }
    }
}

/// Whether `text` may be the program of an engine run by the user `uid`, or why not, in plain
/// words: a full path to a file that can be run, held by `uid` or root, that nobody else can
/// change and that is in no folder anybody else can change. The folders are looked at from
/// `below` down (`/` for inferd itself).
pub(super) fn checked(text: &str, uid: u32, below: &Path) -> Result<(), String> {
    let path = Path::new(text);
    if !path.is_absolute() {
        return Err("Give the whole path of the program, starting with a slash.".to_owned());
    }
    let Ok(meta) = std::fs::metadata(path) else {
        return Err("There is no program at that path.".to_owned());
    };
    if !meta.is_file() {
        return Err("That path is not a program.".to_owned());
    }
    if meta.permissions().mode() & 0o111 == 0 {
        return Err("That file is not allowed to run as a program.".to_owned());
    }
    if !held_under(path, uid, below) {
        return Err(
            "That program, or a folder it is in, belongs to someone else or can be changed by other people, so it is not used."
                .to_owned(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_is_taken_only_when_it_is_a_whole_path_to_a_program_nobody_else_holds() {
        let scratch = test_kit::Scratch::new("program");
        std::fs::set_permissions(&*scratch, std::fs::Permissions::from_mode(0o755))
            .expect("chmod scratch");
        let program = scratch.join("python");
        std::fs::write(&program, "").expect("write");
        let mode = |bits| {
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(bits))
                .expect("chmod");
        };
        let me = rustix::process::geteuid().as_raw();
        let words = |text: &str| checked(text, me, &scratch).expect_err(text);
        assert!(words("python").contains("whole path"));
        assert!(words("./python").contains("whole path"));
        assert!(words(&scratch.join("absent").to_string_lossy()).contains("no program"));
        assert!(words(&scratch.to_string_lossy()).contains("not a program"));
        mode(0o644);
        assert!(words(&program.to_string_lossy()).contains("not allowed to run"));
        // A program the group can change is not taken.
        mode(0o775);
        assert!(words(&program.to_string_lossy()).contains("belongs to someone else"));
        // A program held by the person, in a folder only they can change, is taken.
        mode(0o755);
        assert_eq!(checked(&program.to_string_lossy(), me, &scratch), Ok(()));
        // One that another person holds is not (root holds for everyone, so a test run as root
        // cannot tell).
        if me != 0 {
            assert!(checked(&program.to_string_lossy(), me + 1, &scratch).is_err());
        }
    }

    #[test]
    fn every_program_has_a_row_at_its_own_path_in_the_engines_table() {
        for program in Program::ALL {
            let spec = program.spec();
            assert_eq!(spec.path.0, format!("engines.{}", program.key()));
            assert_eq!(Program::of_path(&spec.path.0), Some(program));
            assert_eq!(spec.kind, KeyKind::Text);
        }
        assert_eq!(Program::of_path("engines.strata_folder"), None);
    }
}
