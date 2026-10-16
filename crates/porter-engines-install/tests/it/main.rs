//! Downloads, installs and the helper, against a fake file host with the scratch CA, a fake
//! package manager and scratch directories: nothing here touches the real system or a network.

mod fetch;
mod helper;
mod install;
mod jobs;
mod support;
