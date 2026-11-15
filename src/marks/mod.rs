//! Important-file marks, separate from maguro.json.
//!
//! The file is marks.json in the config folder (roaming Maguro, or the exe folder when
//! portable.txt is present). Callers pass that directory in. This module never opens maguro.json.

mod store;

#[allow(unused_imports)]
pub(crate) use store::{read_file_identity, FileIdentity, MarksLock, Store, StoreError, CAP, FILE_NAME};
