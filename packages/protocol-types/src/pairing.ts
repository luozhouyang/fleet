import type { Timestamp } from './common';

/** `POST /api/v1/pairing/codes` request. */
export interface CreatePairingCodeRequest {
  org_id?: string;
  machine_name?: string;
}

/** A single-use machine pairing code. The plain text code is returned once. */
export interface PairingCode {
  id: string;
  code: string;
  expires_at: Timestamp;
  org_id: string;
}
