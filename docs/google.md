# Using Google accounts with porter

Porter ships the Google code but no Google client: a Google Cloud OAuth client is a project of
yours, with your name on its consent screen. Until you register one and tell porter its id,
"Google" is in the list of providers and adding it says *Google sign-in needs a client id*.
This page is exactly what to register. It is about 15 minutes in the console, once.

Everything here is Google's console as documented, written without a live check; names of
buttons may have moved. Where a step is marked **(check)**, confirm it against the console.

## 1. A project

1. Open <https://console.cloud.google.com/> and create a project (any name, for example
   `porter`). No billing is needed.
2. In **APIs & Services > Library**, enable each API for the services you will use:

| Service in porter | API to enable |
| --- | --- |
| Calendar | Google Calendar API |
| Contacts | People API |
| Tasks | Google Tasks API |
| Drive app folder | Google Drive API |
| Photos upload and picker | Photos Library API and Photos Picker API |
| Mail (only with `byo = true`, below) | Gmail API (IMAP itself needs no API, but Google asks the project to have it) **(check)** |

An API left off shows in porter as a service the account lacks, for *this build*, not for the
account: enable it and add the account again.

## 2. The consent screen

In **APIs & Services > OAuth consent screen** (**Google Auth Platform > Branding / Audience**
in the newer console) **(check)**:

- **User type: External.** (Internal needs a Workspace organisation.)
- App name and support email: yours. Developer contact: yours.
- **Publishing status: Testing.** Leave it. Verification is a review by Google that takes weeks
  and, for Gmail, a paid security assessment; you do not need it for your own accounts.
- **Test users: add every Google address you will sign in with**, including the one you own.
  Only test users can sign in to an app in testing.
- **Scopes**: add the ones in the next section (the console may call this *Data Access*).

### What testing means

An app in testing mode has two limits you will notice:

- At most 100 test users.
- **Google ends each sign-in after 7 days.** Porter shows the account as needing sign-in and
  says *Google signs this app out every 7 days until it is verified.* Sign in again from
  Settings; it takes a click and the browser. This is the price of not verifying the app. The
  clients.toml row below says `testing = true` so porter knows that is what is happening.

## 3. The scopes, per capability

Porter asks one set of scopes per service and only for services you leave on. They are
*incremental*: a service you turn on later is asked for then, and what you granted before is kept.

| Service | Scope | Class |
| --- | --- | --- |
| Always (who you are) | `openid`, `https://www.googleapis.com/auth/userinfo.email`, `https://www.googleapis.com/auth/userinfo.profile` | non-sensitive |
| Calendar | `https://www.googleapis.com/auth/calendar` | sensitive |
| Contacts | `https://www.googleapis.com/auth/contacts` | sensitive |
| Tasks | `https://www.googleapis.com/auth/tasks` | sensitive |
| Drive, the app folder | `https://www.googleapis.com/auth/drive.appdata` | non-sensitive |
| Photos, upload | `https://www.googleapis.com/auth/photoslibrary.appendonly` | sensitive |
| Photos, picker import | `https://www.googleapis.com/auth/photospicker.mediaitems.readonly` | sensitive |
| Mail (Gmail IMAP and SMTP) | `https://mail.google.com/` | **restricted** |

*Sensitive* scopes show a warning screen ("Google hasn't verified this app") to a client in
testing; click **Advanced > Go to porter**. *Restricted* is Gmail's full-mailbox scope: Google
verifies it only after a security assessment, so it works only for **your own** client with
**your own** test users, which is why mail needs `byo = true` below. Without it porter never asks
for it, and Gmail shows as unavailable for this build.

Porter never asks for the whole of Drive (`.../auth/drive`, restricted) or for the Photos library
(Google removed its read scopes in 2025): Drive is a hidden app folder only porter reads, and
Photos is upload plus the picker you drive in Google's own window.

## 4. The client

In **APIs & Services > Credentials > Create credentials > OAuth client ID**:

- **Application type: Desktop app.** (This is Google's installed-app flow: porter listens on
  `127.0.0.1` on a free port and Google redirects the browser there. No redirect URI is
  registered for a Desktop client; Google accepts any `http://127.0.0.1:<port>`.)
- Name: anything.

Create it and keep the **Client ID** (`123-abc.apps.googleusercontent.com`) and the **Client
secret** (`GOCSPX-...`). Google calls it a secret and still requires it for a desktop client;
it cannot protect anything in an app that runs on your computer, and porter uses PKCE as well.
Porter stores it in your clients file below, never in an account and never on the bus.

## 5. Tell porter

Create or edit `$XDG_CONFIG_HOME/porter/clients.toml` (`~/.config/porter/clients.toml`) and add
this row:

```toml
[[client]]
issuer = "google"
channel = "stable"
client_id = "123-abc.apps.googleusercontent.com"
client_secret = "GOCSPX-..."
testing = true
# byo = true
```

| Key | Meaning |
| --- | --- |
| `issuer` | always `"google"` |
| `channel` | `"stable"` for a release build, `"beta"`, or `"development"` for a development build |
| `client_id`, `client_secret` | from step 4 |
| `testing` | `true` while the consent screen is in Testing (always, unless Google verified it). Porter then knows a sign-in lasts 7 days and says why when it ends |
| `byo` | `true` to also use Gmail through IMAP and SMTP. Omit it unless you want mail: it asks for the restricted scope |

Your file wins over `/usr/share/porter/clients.toml`. The file is read when the account service
starts, so restart it (log out and in, or restart `accountd`) after editing. Settings > Accounts >
Advanced writes only the client id for the issuers it lists, so it will not touch this row;
edit it by hand.

## 6. Add the account

Settings > Accounts > Add > Google (or type your address, `@gmail.com`, `@googlemail.com` or a
domain whose mail Google hosts, and choose Google). The browser opens Google's sign-in; pick the
test user, click through the warning screen, tick every box (porter works with fewer: an unticked
service shows as off), and return. The review lists what the account can do.

## What does not exist yet

Porter signs in and holds the account. Reading and writing the data is separate work: the Google
Calendar source for the calendar mirror, the Drive app folder replica, and Photos upload and
import follow the lanes that need them (see FINDINGS). Signing in now is useful because the grant,
the 7 day reminder and Gmail through the mail app all work today.
