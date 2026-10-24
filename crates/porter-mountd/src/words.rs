//! The words a refusal carries, for the caller to show as they are.

/// The drive cannot be shown here: no way to, a request out of the format, a caller not this
/// person, or a folder that is not a plain folder in Cloud.
pub const NOT_HERE: &str = "Files can't show this drive on this computer.";
/// The drive's folder already holds files of its own, or another folder is shown there.
pub const FOLDER_IN_USE: &str =
    "The drive's folder in Cloud already has other files in it. Move them, then try again.";
/// The drive is shown there already.
pub const ALREADY_SHOWN: &str = "This drive is already shown in Files.";
/// The drive's folder could not be taken away.
pub const NOT_TAKEN_AWAY: &str = "Files couldn't stop showing this drive. Try again later.";
