//! Machine listing and provider discovery.

use fleet_core_protocol::agent::ProviderInfo;
use fleet_core_protocol::machine::{Machine, MachineStatus, RevokedMachine};
use fleet_core_protocol::rpc::methods;

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
        state.connections.disconnect(id);
        state.machines.remove(id).await?;
        Ok(())
    }

    /// List explicitly revoked machines.
    pub async fn revoked(state: &AppState) -> Result<Vec<RevokedMachine>, AppError> {
        Ok(state.db.machines.list_revoked().await?)
    }

    /// Remove a revocation so the machine can pair again.
    pub async fn unrevoke(state: &AppState, id: &str) -> Result<(), AppError> {
        if !state.db.machines.unrevoke(id).await? {
            return Err(not_found("revoked machine", id));
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
