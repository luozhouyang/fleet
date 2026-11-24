//! Machine listing and provider discovery.

use fleet_core_auth::{generate_machine_token, hash_token};
use fleet_core_common::{error_codes, now, ApiError};
use fleet_core_protocol::agent::ProviderInfo;
use fleet_core_protocol::connection::{capabilities, RotateTokenPayload};
use fleet_core_protocol::machine::{Machine, MachineStatus, MachineTokenRotation, RevokedMachine};
use fleet_core_protocol::rpc::methods;
use fleet_core_protocol::PROTOCOL_VERSION;
use serde_json::json;

use crate::error::AppError;
use crate::services::{forward, not_found, parse_response};
use crate::state::AppState;

/// Machine-facing operations exposed by the agent admin.
pub struct MachineService;

impl MachineService {
    /// List known machines, overlaying live connection state on stale rows.
    pub async fn list(state: &AppState) -> Result<Vec<Machine>, AppError> {
        let mut machines = state.machines.list().await?;
        for machine in &mut machines {
            enrich(machine, state);
        }
        Ok(machines)
    }

    /// Fetch one machine or `404`.
    pub async fn get(state: &AppState, id: &str) -> Result<Machine, AppError> {
        let Some(mut machine) = state.machines.get(id).await? else {
            return Err(not_found("machine", id));
        };
        enrich(&mut machine, state);
        Ok(machine)
    }

    /// Revoke a machine, disconnect it, and delete its persistent row.
    /// The revocation record prevents the daemon from silently re-pairing.
    pub async fn remove(state: &AppState, id: &str) -> Result<(), AppError> {
        if state.machines.get(id).await?.is_none() {
            return Err(not_found("machine", id));
        }
        state
            .db
            .machines
            .revoke(id, Some("removed by administrator"))
            .await?;
        state.event_index.mark_machine_revoked(id);
        state.event_index.remove_sessions_for_machine(id);
        state.connections.disconnect(id);
        state.machines.remove(id).await?;
        state.token_rotations.remove(id);
        Ok(())
    }

    /// List explicitly revoked machines visible to `org_id`.
    pub async fn revoked(
        state: &AppState,
        org_id: Option<&str>,
    ) -> Result<Vec<RevokedMachine>, AppError> {
        Ok(state.db.machines.list_revoked_scoped(org_id).await?)
    }

    /// Remove a revocation so the machine can pair again.
    ///
    /// `org_id` scopes the operation for organization administrators; the
    /// legacy super-admin passes `None`. The previous organization is restored
    /// so the machine does not silently move to the default organization.
    pub async fn unrevoke(
        state: &AppState,
        id: &str,
        org_id: Option<&str>,
    ) -> Result<(), AppError> {
        let previous_org = state.db.machines.revocation_org(id).await?;
        let previous_key = state.db.machines.revocation_public_key(id).await?;
        // `unrevoke_scoped` deletes the revocation and clears the token in one
        // transaction, so unrevocation never restores a credential and cannot
        // leave a live token behind after a crash.
        if !state.db.machines.unrevoke_scoped(id, org_id).await? {
            return Err(not_found("revoked machine", id));
        }
        // Unrevocation clears the stored credentials; a plaintext rotation
        // must not outlive them.
        state.token_rotations.remove(id);
        state.event_index.clear_machine_revoked(id);
        // Restore a placeholder row first. The organization mapping has a
        // foreign key to `machines`, and the daemon may reconnect with its
        // pinned key (or a pre-key revocation may have no recorded key).
        if state.db.machines.get(id).await?.is_none() {
            let timestamp = now();
            state
                .db
                .machines
                .upsert(&Machine {
                    id: id.to_string(),
                    name: id.to_string(),
                    hostname: String::new(),
                    os: String::new(),
                    arch: String::new(),
                    daemon_version: String::new(),
                    protocol_version: PROTOCOL_VERSION,
                    status: MachineStatus::Unknown,
                    created_at: timestamp,
                    updated_at: timestamp,
                    connected_at: None,
                    last_seen_at: None,
                    providers: Vec::new(),
                    workspaces: Vec::new(),
                    workspace_count: 0,
                    public_key: previous_key,
                })
                .await?;
            // `MachineRegistry::list` serves the warm cache; load the restored
            // row so it appears immediately.
            state.machines.get(id).await?;
        }
        if let Some(previous_org) = &previous_org {
            state.db.machine_orgs.ensure(id, previous_org).await?;
        }
        Ok(())
    }

    /// Providers advertised by a machine.
    ///
    /// When the machine is connected the daemon is asked for its current
    /// capabilities; otherwise the last persisted snapshot is returned so the
    /// dashboard keeps working while a machine is offline.
    pub async fn providers(state: &AppState, id: &str) -> Result<Vec<ProviderInfo>, AppError> {
        let machine = Self::get(state, id).await?;
        if state.connections.is_online(id) {
            let value = forward(state, id, methods::PROVIDERS_LIST, serde_json::json!({})).await?;
            parse_response(value, "provider list")
        } else {
            Ok(machine.providers)
        }
    }

    /// Generate a token and record its hash as pending.
    ///
    /// Returns `machine_token_rotation_pending` when an earlier rotation was
    /// never confirmed, so two different tokens can never be in flight at once.
    async fn begin_rotation(state: &AppState, id: &str) -> Result<String, AppError> {
        if state.db.machines.pending_token_hash(id).await?.is_some() {
            return Err(AppError::from_api(ApiError::new(
                error_codes::MACHINE_TOKEN_ROTATION_PENDING,
                "a previous token rotation is unconfirmed; wait for the daemon to \
                 reconnect or cancel the rotation first",
            )));
        }
        let token = generate_machine_token();
        if !state
            .db
            .machines
            .set_pending_token(id, &hash_token(&token))
            .await?
        {
            return Err(not_found("machine", id));
        }
        state.token_rotations.insert(id, token.clone());
        Ok(token)
    }

    /// Rotate a machine token over its authenticated connection.
    ///
    /// The new hash is stored as pending before the daemon sees the token, so
    /// both credentials authenticate until the daemon confirms. On success the
    /// pending hash is promoted and the previous token stops working. On
    /// failure both the pending hash and the plaintext are kept: the daemon may
    /// have persisted the token before the acknowledgement was lost, and a
    /// retry resends the same token.
    pub async fn rotate_token(
        state: &AppState,
        actor: &str,
        org_id: Option<&str>,
        id: &str,
    ) -> Result<MachineTokenRotation, AppError> {
        if state.machines.get(id).await?.is_none() {
            return Err(not_found("machine", id));
        }
        if !state.db.machines.has_token(id).await? {
            return Err(AppError::from_api(ApiError::new(
                error_codes::MACHINE_NOT_PAIRED,
                "machine has no token to rotate; pair it first",
            )));
        }
        // Refuse before creating pending state when the token cannot be
        // delivered: an offline or too-old daemon would otherwise leave a
        // rotation that can only be resolved by a later reconnect or cancel.
        if !state.connections.is_online(id) {
            return Err(AppError::machine_offline(id));
        }
        if !state.connections.supports(id, capabilities::TOKEN_ROTATION) {
            return Err(AppError::from_api(ApiError::new(
                error_codes::UNSUPPORTED,
                "the connected daemon does not support token rotation; upgrade fleet-daemon first",
            )));
        }

        let token = match state.token_rotations.get(id) {
            Some(token)
                if state.db.machines.pending_token_hash(id).await?.as_deref()
                    == Some(hash_token(&token).as_str()) =>
            {
                token
            }
            Some(_) => {
                // The stored rotation was resolved (paired, revoked, or
                // canceled) while its plaintext lingered. Never resend a token
                // that is no longer pending: the daemon would save it and the
                // admin would reject it on the next reconnect.
                state.token_rotations.remove(id);
                Self::begin_rotation(state, id).await?
            }
            None => Self::begin_rotation(state, id).await?,
        };

        let payload = serde_json::to_value(RotateTokenPayload {
            machine_token: token.clone(),
        })
        .map_err(AppError::internal)?;
        match state
            .connections
            .request_with_timeout(id, methods::ROTATE_TOKEN, payload, state.request_timeout())
            .await
        {
            Ok(_) => {
                let promoted = state
                    .db
                    .machines
                    .promote_pending_token(id, &hash_token(&token))
                    .await?;
                state.token_rotations.remove(id);
                if !promoted {
                    // Either the daemon reconnected with the pending token
                    // before the acknowledgement arrived (the handshake
                    // promoted it), or a concurrent cancel, re-pair or
                    // revocation resolved it. Reporting an error here would
                    // invite a retry that replaces the only token the daemon
                    // can use, so the rotation still reports success.
                    tracing::warn!(
                        machine = %id,
                        "machine token rotation was resolved outside this request"
                    );
                }
                audit(
                    state,
                    actor,
                    org_id,
                    "machine.token_rotate",
                    id,
                    json!({"confirmed": true}),
                )
                .await;
                Ok(MachineTokenRotation {
                    machine_id: id.to_string(),
                    rotated_at: now(),
                })
            }
            Err(error) => {
                audit(
                    state,
                    actor,
                    org_id,
                    "machine.token_rotate",
                    id,
                    json!({"confirmed": false, "error": error.code}),
                )
                .await;
                let code = match error.code.as_str() {
                    error_codes::MACHINE_OFFLINE | error_codes::TIMEOUT => error.code.clone(),
                    _ => error_codes::MACHINE_TOKEN_ROTATION_FAILED.to_string(),
                };
                Err(AppError::from_api(ApiError::new(
                    code,
                    format!(
                        "the daemon did not confirm the token rotation: {}",
                        error.message
                    ),
                )))
            }
        }
    }

    /// Abandon an unconfirmed rotation. The current token stays valid.
    pub async fn cancel_token_rotation(
        state: &AppState,
        actor: &str,
        org_id: Option<&str>,
        id: &str,
    ) -> Result<(), AppError> {
        if state.machines.get(id).await?.is_none() {
            return Err(not_found("machine", id));
        }
        let had_pending = state.db.machines.pending_token_hash(id).await?.is_some();
        state.db.machines.clear_pending_token(id).await?;
        state.token_rotations.remove(id);
        if had_pending {
            audit(
                state,
                actor,
                org_id,
                "machine.token_rotate_cancel",
                id,
                json!({}),
            )
            .await;
        }
        Ok(())
    }
}

/// Append an audit record without failing the operation; the audit trail is
/// best-effort once the credential has already changed.
async fn audit(
    state: &AppState,
    actor: &str,
    org_id: Option<&str>,
    action: &str,
    subject: &str,
    details: serde_json::Value,
) {
    if let Err(error) = state
        .db
        .audit
        .append(actor, org_id, action, subject, details)
        .await
    {
        tracing::warn!(error = %error, action, subject, "failed to audit machine operation");
    }
}

/// Overlay live connection information onto persisted machine metadata.
fn enrich(machine: &mut Machine, state: &AppState) {
    // A machine that completed the key handshake but has not yet delivered its
    // registration snapshot must not be reported as online: providers and
    // workspaces would be empty mid-registration.
    if machine.status == MachineStatus::Unknown {
        return;
    }
    if state.connections.is_online(&machine.id) {
        machine.status = MachineStatus::Online;
        if let Some(last_seen) = state.connections.last_seen(&machine.id) {
            machine.last_seen_at = Some(last_seen);
        }
    } else if machine.status.is_online() {
        // The persisted row still says online but no live socket exists.
        machine.status = MachineStatus::Offline;
    }
}
