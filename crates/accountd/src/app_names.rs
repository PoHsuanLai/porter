//! What Settings calls an app: the name a person reads in the account's Apps group, not the bus
//! id the grant is keyed by.
//!
//! Resolved in this order: the caller table's `name` for the app, then `Name=` of the
//! `[Desktop Entry]` group of `<app id>.desktop` in the first of the application directories
//! that has one (`$XDG_DATA_HOME/applications`, then each `$XDG_DATA_DIRS` entry's), else the
//! app id itself.
//!
//! The desktop entries are read lazily, once per label when the settings schema is built: that
//! is a few small files per app that holds a grant, only when Settings asks (`Describe`), so
//! nothing is cached and an app installed or renamed shows its new name on the next read. Only
//! the unlocalised `Name=` is read; `Name[xx]=` is not (FINDINGS).

use porter_core::capability::AgentProgram;
use porter_core::{AppLabel, AppName, AuthKind, Capability};
use porter_dbus::{AppTitle, CallerTable};
use porter_provider::ProviderSpec;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// The bus-id prefix of the app an agent program's grants are held under.
const AGENT_APP_PREFIX: &str = "org.quire.Agent.";

/// Where an app's display name comes from.
#[derive(Debug, Clone, Default)]
pub struct AppNames {
    table: CallerTable,
    applications: Vec<PathBuf>,
    agents: BTreeMap<AgentProgram, String>,
}

impl AppNames {
    /// Names from `table`'s rows, then from desktop entries in `applications` (the `applications`
    /// directories, the person's before the system's).
    pub fn new(table: CallerTable, applications: Vec<PathBuf>) -> Self {
        Self {
            table,
            applications,
            agents: BTreeMap::new(),
        }
    }

    /// Also name the agent apps `org.quire.Agent.<program>` after the label of the provider file
    /// that runs `<program>` and signs in itself (`agent_login`): "Claude Code" for
    /// `claude-code.toml`. A key provider that merely names the program (anthropic.toml) does
    /// not name the app. A program with no such file stays its id.
    pub fn with_agents(mut self, specs: &[ProviderSpec]) -> Self {
        self.agents = agent_labels(specs);
        self
    }

    /// What a person reads for `app`.
    pub fn title_of(&self, app: &AppName) -> AppTitle {
        self.named(app)
            .unwrap_or_else(|| AppTitle(app.as_str().to_owned()))
    }

    /// The name accountd has for `app`, if it has one: the id itself is not a name, so an app
    /// nothing names is `None` and the sheet host keeps naming it as it can.
    pub fn label_of(&self, app: &AppName) -> Option<AppLabel> {
        self.named(app).map(|title| AppLabel(title.0))
    }

    fn named(&self, app: &AppName) -> Option<AppTitle> {
        self.table
            .title_of(app)
            .cloned()
            .or_else(|| self.entry_title(app))
            .or_else(|| self.agent_title(app))
    }

    fn agent_title(&self, app: &AppName) -> Option<AppTitle> {
        let program = AgentProgram::parse(app.as_str().strip_prefix(AGENT_APP_PREFIX)?).ok()?;
        self.agents.get(&program).cloned().map(AppTitle)
    }

    fn entry_title(&self, app: &AppName) -> Option<AppTitle> {
        let file = format!("{}.desktop", app.as_str());
        self.applications
            .iter()
            .find_map(|dir| std::fs::read_to_string(dir.join(&file)).ok())
            .and_then(|text| entry_name(&text))
    }
}

/// Each agent program the specs run under an `agent_login` provider, with that provider's label.
fn agent_labels(specs: &[ProviderSpec]) -> BTreeMap<AgentProgram, String> {
    specs
        .iter()
        .filter(|spec| spec.auth.kind == AuthKind::AgentLogin)
        .flat_map(|spec| {
            spec.capabilities
                .iter()
                .filter_map(|row| match &row.capability {
                    Capability::Agent(agent) => Some((agent.program.clone(), spec.label.clone())),
                    _ => None,
                })
        })
        .collect()
}

/// `Name=` of the `[Desktop Entry]` group of `text`, if it is there and not empty.
fn entry_name(text: &str) -> Option<AppTitle> {
    let mut in_entry = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
        } else if let Some(name) = line.strip_prefix("Name=").filter(|_| in_entry) {
            let name = name.trim();
            return (!name.is_empty()).then(|| AppTitle(name.to_owned()));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use porter_dbus::{CallerRole, CallerRow};
    use std::path::Path;

    fn app(text: &str) -> AppName {
        AppName::parse(text).expect("name")
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("app-names-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        dir
    }

    fn entry(dir: &Path, id: &str, text: &str) {
        std::fs::write(dir.join(format!("{id}.desktop")), text).expect("entry");
    }

    fn table(id: &str, title: &str) -> CallerTable {
        CallerTable {
            callers: vec![CallerRow {
                app: app(id),
                unit: None,
                role: CallerRole::App,
                name: Some(AppTitle(title.to_owned())),
            }],
        }
    }

    #[test]
    fn the_caller_table_name_wins_over_a_desktop_entry() {
        let dir = scratch("table");
        entry(
            &dir,
            "org.quire.Sync",
            "[Desktop Entry]\nName=From the entry\n",
        );
        let names = AppNames::new(table("org.quire.Sync", "Sync"), vec![dir.clone()]);
        assert_eq!(names.title_of(&app("org.quire.Sync")).0, "Sync");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_desktop_entry_names_an_app_the_table_does_not_list() {
        let dir = scratch("entry");
        entry(
            &dir,
            "org.example.Photos",
            "# a comment\n[Desktop Entry]\nType=Application\nName[de]=Fotos\nName= Photos \nExec=photos\n\n\
             [Desktop Action new]\nName=New window\n",
        );
        let names = AppNames::new(table("org.quire.Sync", "Sync"), vec![dir.clone()]);
        assert_eq!(names.title_of(&app("org.example.Photos")).0, "Photos");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn only_the_desktop_entry_group_names_the_app() {
        let dir = scratch("group");
        entry(
            &dir,
            "org.example.Other",
            "[Desktop Action new]\nName=New window\n[Desktop Entry]\nExec=other\n",
        );
        entry(&dir, "org.example.Empty", "[Desktop Entry]\nName=\n");
        let names = AppNames::new(CallerTable::default(), vec![dir.clone()]);
        assert_eq!(
            names.title_of(&app("org.example.Other")).0,
            "org.example.Other"
        );
        assert_eq!(
            names.title_of(&app("org.example.Empty")).0,
            "org.example.Empty"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_first_directory_that_has_the_entry_wins() {
        let (home, system) = (scratch("home"), scratch("system"));
        entry(&home, "org.example.A", "[Desktop Entry]\nName=Mine\n");
        entry(&system, "org.example.A", "[Desktop Entry]\nName=Theirs\n");
        entry(
            &system,
            "org.example.B",
            "[Desktop Entry]\nName=Only theirs\n",
        );
        let names = AppNames::new(CallerTable::default(), vec![home.clone(), system.clone()]);
        assert_eq!(names.title_of(&app("org.example.A")).0, "Mine");
        assert_eq!(names.title_of(&app("org.example.B")).0, "Only theirs");
        let _ = std::fs::remove_dir_all(home);
        let _ = std::fs::remove_dir_all(system);
    }

    #[test]
    fn an_agent_app_is_named_by_its_agent_provider_file() {
        let names = AppNames::default().with_agents(&porter_provider::shipped_specs());
        assert_eq!(
            names.title_of(&app("org.quire.Agent.claude-code")).0,
            "Claude Code"
        );
    }

    #[test]
    fn an_agent_with_no_provider_file_stays_its_id_and_a_table_name_still_wins() {
        let names = AppNames::new(
            table("org.quire.Agent.claude-code", "Mine"),
            vec![scratch("agents")],
        )
        .with_agents(&porter_provider::shipped_specs());
        assert_eq!(
            names.title_of(&app("org.quire.Agent.claude-code")).0,
            "Mine"
        );
        assert_eq!(
            names.title_of(&app("org.quire.Agent.nobody")).0,
            "org.quire.Agent.nobody"
        );
        let none = AppNames::default().with_agents(&[]);
        assert_eq!(
            none.title_of(&app("org.quire.Agent.claude-code")).0,
            "org.quire.Agent.claude-code"
        );
    }

    #[test]
    fn an_app_nothing_names_is_its_id() {
        let dir = scratch("none");
        let names = AppNames::new(CallerTable::default(), vec![dir.clone()]);
        assert_eq!(
            names.title_of(&app("org.example.Ghost")).0,
            "org.example.Ghost"
        );
        assert_eq!(
            AppNames::default().title_of(&app("org.example.Ghost")).0,
            "org.example.Ghost"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
