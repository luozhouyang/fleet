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
mod machine_token;
mod pairing;
mod token;

pub use challenge::{new_challenge, nonce_base64, Challenge, NONCE_LEN};
pub use error::{AuthError, Result};
pub use identity::{verify_signature, MachineIdentity};
pub use machine_token::{
    clear_machine_token, load_machine_token, load_or_create_machine_id, machine_id_path,
    machine_token_path, save_machine_token,
};
pub use pairing::{
    generate_machine_token, generate_pair_code, hash_pair_code, normalize_pair_code, PAIR_CODE_LEN,
};
pub use token::{
    admin_token_env, generate_user_token, hash_token, TokenAuth, ADMIN_TOKEN_ENV, DEV_MODE_ENV,
    DEV_TOKEN,
};
