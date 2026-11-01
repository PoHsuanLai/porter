//! The shell's watch of accounts through the client (`Accounts::watch`), against the real
//! accountd front end on a private bus: an account that is refused, and the same account when it
//! is well again, are both changes of its state.

use crate::common::{Rig, SheetHost, caller, mail_account, poking_options, publish};
use porter_callers::CallerRole;
use porter_client::{AccountChange, AccountRef, Changes, Porter};
use porter_client_dbus::Bus;
use porter_core::AccountState;

/// The next change, within the generous wait.
async fn told(changes: &mut Changes<AccountChange>) -> AccountChange {
    tokio::time::timeout(test_kit::GENEROUS, changes.next())
        .await
        .expect("told in time")
        .expect("a change")
        .expect("well formed")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_recovering_account_is_told_as_a_state_like_a_refused_one() {
    let (_dir, options) = poking_options();
    let rig = Rig::start_with(options, SheetHost::quiet()).await;
    let mail = mail_account();
    let shell = rig
        .client_as(caller("org.quire.Shell", CallerRole::SheetHost))
        .await;
    let accounts = Porter::over(Bus::over(shell)).accounts();
    let mut changes = accounts.watch().await.expect("watch");
    let account = AccountRef::of_account(&mail.id);

    assert!(
        rig.service
            .set_state(&mail.id, AccountState::NeedsReauth)
            .await
    );
    publish(&rig).await;
    assert_eq!(
        told(&mut changes).await,
        AccountChange::State(account.clone(), AccountState::NeedsReauth)
    );

    assert!(rig.service.set_state(&mail.id, AccountState::Ok).await);
    publish(&rig).await;
    assert_eq!(
        told(&mut changes).await,
        AccountChange::State(account, AccountState::Ok)
    );
}
