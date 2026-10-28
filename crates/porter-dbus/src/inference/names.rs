//! inferd's bus name and object, the option and result keys its methods take and give, and the
//! error-name prefixes its refusals use.

/// inferd's bus name.
pub const INFERENCE_BUS: &str = "org.quire.Inference1";
/// inferd's object.
pub const INFERENCE_PATH: &str = "/org/quire/Inference1";
/// inferd's settings module (`org.quire.SettingsModule1`, declared in design/22 §9.4, not
/// here): the picker rows per kind and the `ai.model.<kind>.<tier>` map.
pub const INFERENCE_SETTINGS_PATH: &str = "/org/quire/Inference1/settings";
/// The reserved key of the W3C trace context in an `options` vardict (`Inference1.Open`,
/// `Prepare`, `Availability`, and the same dictionary on the companion and intents interfaces).
/// The value is a version 00 `traceparent` string (`porter_infer::Traceparent`); a daemon that
/// finds none starts its own root, and an unknown key is ignored.
pub const OPTION_TRACEPARENT: &str = "traceparent";
/// The reserved key of the session's usage in the same `options` vardict (`Inference1.Open`,
/// `Prepare`, `Availability`): whether a person is waiting on it or nobody is. The value is a
/// string, `porter_core::consent::Usage`'s slug (`interactive`, `background`); inferd asks
/// accountd's `Verdicts` with it. Absent means `interactive`; an unknown slug is refused as
/// invalid args.
pub const OPTION_USAGE: &str = "usage";
/// The key, in the same `options` vardict, of the places a call may run: an `as` of place ids
/// (`this-computer`, `computer:<name>`, `account:<account id>`) in the caller's order of
/// preference. Only docket's companion, reader and intents daemons may send it (by unit); any
/// other caller is refused `AccessDenied`, not ignored. With it inferd routes inside the set and
/// never outside it; the data class's floor and the local-only switch still apply.
pub const OPTION_PLACES: &str = "places";
/// The key of the model to use at a place of the set: an `a{ss}` of place id to model id. A place
/// with no entry uses the usual choice. Sent only with [`OPTION_PLACES`].
pub const OPTION_PLACE_MODELS: &str = "place_models";
/// The key of whose turn the session carries: a string, `person` or `helper:<id>`
/// (`porter_infer::Asker::to_text`). Absent means the person. Only docket's companion, reader
/// and intents daemons may send it; any other caller is refused `AccessDenied`, not ignored.
pub const OPTION_ASKER: &str = "asker";
/// The key of the session's own named model: a string, `<account>/<model>`
/// (`porter_infer::ModelRef`). It is treated like a model the person named: never overridden,
/// a refusal names it, no other model stands in. Only docket's daemons may send it, as for
/// [`OPTION_ASKER`].
pub const OPTION_MODEL: &str = "model";
/// The keys of one row of `Inference1.HardwareNotes` (`a(sa{sv})`: the note's kind slug, then
/// these). `words` (`s`) is the sentence to show; the rest are the note's numbers, present where
/// the note has them (`t` each): `helpers`, `ahead`, and `free` and `one_more` in bytes.
pub const NOTE_KEY_WORDS: &str = "words";
/// The keys of a row of `Inference1.Places` (`a(sa{sv})`: the place's id, then these). `kind`
/// is `this_computer`, `own_computer` or `cloud_account` (`s`); `name` is the computer's name or
/// the account's label (`s`); `provider` is the provider's display name, on a cloud account only
/// (`s`); `models` lists the models the place can serve now, as `a(ss)` of model id and display
/// name; `ready` says whether the place can serve now (`b`), for a program to read and never to
/// put into a sentence a person sees.
pub const PLACE_KEY_KIND: &str = "kind";
/// The keys of one model's vardict in `Inference1.AddComputer`: `socket` (`s`) is the full path of
/// the connection on this computer that leads to the engine, `port` (`q`) the port on this
/// computer that does (give one of the two), and `key` (`s`) the engine's key when it wants one.
pub const COMPUTER_KEY_SOCKET: &str = "socket";
/// See [`COMPUTER_KEY_SOCKET`].
pub const COMPUTER_KEY_PORT: &str = "port";
/// See [`COMPUTER_KEY_SOCKET`].
pub const COMPUTER_KEY_KEY: &str = "key";
/// The prefix of the errors `AddComputer`, `RemoveComputer`, `AddTailnetComputer`, `AnswerGuest`
/// and `ForgetGuest` answer with: `org.quire.Inference1.Error.Computer.<Name>`, the name one of
/// `BadName`, `NoModels`, `TooMany`, `AlreadyThere`, `UnknownModel`, `ModelNotUsable`,
/// `ModelTaken`, `BadAddress`, `BadKey`, `NotSaved`, `NotThere`, `AddedByHand`, `Unavailable`,
/// and, since `AddTailnetComputer` and the guests: `NotOnTailscale`, `NotAnswering`,
/// `NotAsking`, `TooManyAsking`. The reply's text is a plain sentence for the person.
pub const COMPUTER_ERROR_PREFIX: &str = "org.quire.Inference1.Error.Computer.";
/// The keys of a row of `Inference1.Candidates` (`a(sa{sv})`: the computer's Tailscale node id,
/// then these). `name` is the name people call the computer by (`s`); `models` lists the models
/// it lends, as `a(ss)` of model id and display name; `needs_approval` says whether the person
/// still has to say yes on that computer (`b`).
pub const CANDIDATE_KEY_NAME: &str = "name";
/// See [`CANDIDATE_KEY_NAME`].
pub const CANDIDATE_KEY_MODELS: &str = "models";
/// See [`CANDIDATE_KEY_NAME`].
pub const CANDIDATE_KEY_NEEDS_APPROVAL: &str = "needs_approval";
/// The keys of a row of `Inference1.Guests` (`a(sa{sv})`: the computer's node id, then these)
/// and of the details of the `GuestAsks` signal (`name` and `since` only). `name` is the name
/// people call the computer by (`s`); `state` is `approved`, `denied` or `asking` (`s`); `since`
/// is when the person answered, or when it began asking (`x`, Unix seconds).
pub const GUEST_KEY_NAME: &str = "name";
/// See [`GUEST_KEY_NAME`].
pub const GUEST_KEY_STATE: &str = "state";
/// See [`GUEST_KEY_NAME`].
pub const GUEST_KEY_SINCE: &str = "since";
/// The words `Inference1.AnswerGuest` takes as its `answer` (`s`): let the computer use this
/// computer's models, or keep it out. Any other word is invalid args.
pub const GUEST_ANSWER_ALLOW: &str = "allow";
/// See [`GUEST_ANSWER_ALLOW`].
pub const GUEST_ANSWER_DENY: &str = "deny";
/// See [`PLACE_KEY_KIND`].
pub const PLACE_KEY_NAME: &str = "name";
/// See [`PLACE_KEY_KIND`].
pub const PLACE_KEY_PROVIDER: &str = "provider";
/// See [`PLACE_KEY_KIND`].
pub const PLACE_KEY_MODELS: &str = "models";
/// See [`PLACE_KEY_KIND`].
pub const PLACE_KEY_READY: &str = "ready";
/// See [`PLACE_KEY_KIND`]. `a(sss)`: for each model of `models` whose engine is known, its id, the
/// engine's id and the engine's name; a model with no entry has no known engine.
pub const PLACE_KEY_ENGINES: &str = "engines";
/// See [`PLACE_KEY_KIND`]. `a(ss)`: for each model of `models` whose input is known, its id and
/// what it takes in (`text_only`, `text_and_images`, `text_and_audio`, `text_images_and_audio`);
/// a model with no entry has no known input, and a name a reader does not know reads as unknown.
pub const PLACE_KEY_TAKES: &str = "takes";
