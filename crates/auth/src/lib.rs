//! Authentication primitives for the Agent Fleet agent admin and machine daemon.
//!
//! This crate provides three building blocks:
//!
//! - [`MachineIdentity`]: an Ed25519 key pair with a versioned on-disk key file
//!   and `verify_strict` signature verification, used to prove machine identity.
//! - [`TokenAuth`]: constant-time bearer-token authentication for HTTP APIs.
//! - [`Challenge`]: random, unique nonces for replay-protected handshakes.
//!
//! Machine identity and user authorization are deliberately separate concerns;
//! possession of a valid machine key never grants access to another machine's
//! workspaces or sessions.

#![warn(missing_docs)]

mod challenge;
mod error;
mod identity;
mod token;

pub use challenge::{new_challenge, nonce_base64, Challenge, NONCE_LEN};
pub use error::{AuthError, Result};
pub use identity::{verify_signature, MachineIdentity};
pub use token::{
    admin_token_env, generate_user_token, hash_token, machine_token_env, TokenAuth,
    ADMIN_TOKEN_ENV, DEV_MODE_ENV, DEV_TOKEN, MACHINE_TOKEN_ENV,
};
