//! The fake of `org.quire.Storage1` (syncd-fakes, which the rig serves to Files) says the words
//! the live service says.

use porter_drive::Reason;
use syncd::words;
use syncd_fakes::storage::words as fake;

#[test]
fn the_fake_of_the_service_gives_the_words_of_the_live_one() {
    for (reason, said) in [
        (Reason::NotOnADrive, fake::NOT_ON_A_DRIVE),
        (Reason::OnlineOnly, fake::ONLINE_ONLY),
        (Reason::Offline, fake::OFFLINE),
        (Reason::Waiting, fake::WAITING),
        (Reason::Conflict, fake::CONFLICT),
        (Reason::KeptWithFolder, fake::KEPT_WITH_FOLDER),
        (Reason::NoDrive, fake::NO_DRIVE),
        (Reason::NotADocument, fake::NOT_A_DOCUMENT),
        (Reason::NoFileFormat, fake::NO_FILE_FORMAT),
    ] {
        assert_eq!(words::reason(reason), said, "{reason:?}");
    }
}
