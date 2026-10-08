# porter

Porter keeps the accounts on your computer in one place. You sign in to a service once (mail,
calendar, contacts, files, photos, an AI service), and the apps you choose can then use that
account without ever seeing your password or key. Every app asks first, you decide what each one
may do, and you can take it back at any time.

It is for people who use a desktop made of many small apps and do not want each of them to ask
for the same password, keep its own copy of it, or reach further than it needs to.

Status: `0.1.0-beta.1`. See [CHANGELOG.md](CHANGELOG.md) for what is in it and what has not yet met
the real services.

## What runs

Porter is three background services. They start when an app first needs them and stop when
nothing does.

| Service | Name on the session bus | What it does |
| --- | --- | --- |
| `accountd` | `org.quire.Accounts1` | Holds the accounts, the consent you gave each app and the audit trail. Signs you in, keeps the passwords and keys in your keyring, and gives apps a short-lived pass or a connection already signed in, never the secret itself. |
| `syncd` | `org.quire.Sync1` | Keeps copies of calendars, contacts, tasks, files and photos in step with the service, for the apps that hold a grant. |
| `inferd` | `org.quire.Inference1` | The AI service. Lets apps use a model on this computer or one you have an account for, under the limits you set (what may leave the machine, how much it may spend). |

The windows you see (the sign-in sheet, the Settings pages) belong to the desktop, not to porter.
Porter supplies what they show.

## Install

You need a Linux desktop with systemd and a keyring (GNOME Keyring or KWallet), and a Rust
toolchain to build.

    scripts/install.sh                      # builds, then installs under /usr (run as root)
    scripts/install.sh --prefix /usr/local
    scripts/install.sh --destdir /tmp/stage # copies into a staging folder and touches nothing else

The script also reads `PREFIX` and `DESTDIR` from the environment. It can be run again at any
time: a file that is already right is left alone, and a caller table you edited is kept (the
shipped one is put beside it as `callers.toml.dist`). It never writes into a home folder.
`--bin-dir DIR` installs services you built yourself instead of building them, and `--help` lists
the rest.

Building needs access to the private repositories that porter's dependencies come from (stoker
and quire, as git dependencies); there is no public mirror yet.

## Where files go

Installed by the script (under the prefix, `/usr` by default):

| What | Where |
| --- | --- |
| The three services | `libexec/quire/` |
| Their systemd user units | `lib/systemd/user/` |
| What lets the session bus start them | `share/dbus-1/services/` |
| The providers porter knows (one file each) | `share/porter/providers/` |
| The sign-in client ids for Microsoft and others, when some ship | `share/porter/clients.toml` |
| The AI page's settings description | `share/quire/settings/inferd.settings.toml` |
| Examples to copy: the AI service's configuration and the opt-in network drop-in | `share/doc/porter/examples/` |
| The table that says which app is Settings, which draws the sheets, which are the services | `/etc/porter/callers.toml` |

Yours, made as you use porter:

| What | Where |
| --- | --- |
| The accounts and your consent | `$XDG_STATE_HOME/porter/` (by default `~/.local/state/porter/`) |
| The audit trail | `$XDG_STATE_HOME/quire/accountd/` |
| Your own provider files, which win over the shipped ones | `$XDG_DATA_HOME/porter/providers/` |
| Your own client ids (Settings writes this), and callers | `$XDG_CONFIG_HOME/porter/` |
| Synced copies (calendars, contacts, files, photos) | `$XDG_DATA_HOME/porter/` |
| The AI service's configuration | `$XDG_CONFIG_HOME/quire/inferd.toml` |

Secrets (passwords, keys, tokens) are in your keyring, never in these files.

## Signing in to Google and Microsoft

These two need a sign-in client id registered with the company. Google's has to be your own: see
[docs/google.md](docs/google.md). Until a client id is set, Settings says so on that row.

## Documentation

- [ARCHITECTURE.md](ARCHITECTURE.md): the crates, what depends on what, what is built and what is not.
- [CONVENTIONS.md](CONVENTIONS.md): how the code is written and tested.
- [FINDINGS.md](FINDINGS.md): open items, standing facts and what each piece of work found.
- [docs/google.md](docs/google.md): registering a Google client and what porter does with each Google service.
- [CHANGELOG.md](CHANGELOG.md).

## Working on porter

    cargo test --workspace        # the tests that start a private session bus are meant to run in a jail (no real session, devices or network); .github/workflows/ci.yml says which packages
    scripts/check-boundary.sh     # the crate boundaries, mechanically
    cargo deny check licenses

## Licence

MIT or Apache-2.0, at your option: [LICENSE-MIT](LICENSE-MIT), [LICENSE-APACHE](LICENSE-APACHE).
