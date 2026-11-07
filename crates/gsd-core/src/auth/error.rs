//! Errors returned by the auth service. The HTTP layer maps each variant to a stable API code.

use crate::{auth::password::PasswordError, random::RandomError, store::RepoError};

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("invalid username or password")]
    InvalidCredentials,
    #[error("initial setup has already been completed")]
    AlreadySetUp,
    #[error("not signed in, or the session has expired")]
    SessionInvalid,
    #[error("invalid username: {0}")]
    InvalidUsername(&'static str),
    #[error("password rejected: {0}")]
    WeakPassword(&'static str),
    #[error("that username is already taken")]
    UsernameTaken,
    #[error("user not found")]
    UserNotFound,
    #[error("you can't change or delete your own account here")]
    OwnAccount,
    #[error("the last owner can't be demoted, disabled or deleted")]
    LastOwner,
    #[error(transparent)]
    Repo(#[from] RepoError),
    #[error(transparent)]
    Password(#[from] PasswordError),
    #[error(transparent)]
    Random(#[from] RandomError),
    #[error("a background task failed unexpectedly")]
    BackgroundTask,
}
