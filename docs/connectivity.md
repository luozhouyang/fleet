# Connectivity and Machine Enrollment

This document covers two related problems:

1. **How a daemon joins an admin safely** — short-lived pairing codes plus a
   per-machine token (implemented for same-host / same-LAN deployments).
2. **How a daemon reaches the admin across LAN, NAT and the public internet** —
   why the reverse WebSocket design is still right, and which transport,
   discovery and fallback strategies fit which deployment.

The guiding principle is: **the daemon only makes outbound connections**. It
never listens for admin traffic. This keeps daemons usable behind NAT,
corporate firewalls and home routers, and keeps their attack surface small.

---

## 1. Current machine authentication (implemented)

A daemon enrolls once and then connects with credentials that are unique to it:

1. An admin creates a single-use pairing code bound to an organization (and an
   optional machine-name hint). The plaintext code is returned once; only its
   SHA-256 hash is stored.
2. `af setup` (or `fleet-daemon pair` after `af machines pair-code`) loads or
   creates the Ed25519 key pair (`~/.agent-fleet/machine.key`), sends
   `connection.hello` with the key and code, and signs the admin's challenge.
3. The admin validates the code, verifies the signature, pins the public key,
   consumes the code atomically and returns a per-machine token
   (`afm_<64 hex>`, hash-only at rest).
4. The daemon writes the token to `<key_file>.token` with mode 0600. Every later
   connection presents `Authorization: Bearer <machine-token>` and signs a fresh
   challenge. Configuration never contains the token.

What is gone: the shared machine token, the daemon-config token field, the
key-only mode, and trust-on-first-use. A machine with
no pin and no code is rejected with `machine_not_paired`; a pinned machine
without a token with `machine_token_required`; a changed key needs a rotation
proof signed by the previously pinned key (`machine_key_mismatch` otherwise).

**Revocation behavior.** `DELETE /api/v1/machines/{id}` writes a permanent
`machine_revocations` record, disconnects the live connection and removes the
active machine row. A revoked daemon is rejected during the handshake and stops
reconnecting instead of backing off indefinitely.
`GET /api/v1/machines/revoked` lists revocations and
`POST /api/v1/machines/{id}/unrevoke` (or `af machines unrevoke <id>`) restores
the org scope and pinned key. A revoked machine cannot pair again until it is
unrevoked, even if it presents a fresh code.

### Enrollment commands

```bash
# One command: create a code, pair this machine, write the daemon config,
# store <key_file>.token, and remove token/pairing_code from the TOML.
af --url http://127.0.0.1:8787 --token <admin-token> setup \
  --org org_default \
  --server ws://127.0.0.1:8787/ws/machine \
  --daemon-config /etc/fleet/daemon.toml \
  --name workstation

# Split flow: create the code on an admin host, redeem it on the machine.
af --url http://127.0.0.1:8787 --token <admin-token> machines pair-code \
  --org org_default --name workstation
fleet-daemon pair --server ws://127.0.0.1:8787/ws/machine --code <code> \
  --config /etc/fleet/daemon.toml
```

`af setup --configure-only` skips code creation/pairing and only writes config.
Plaintext `ws://` is refused for non-loopback hosts unless `--allow-insecure`
is passed on a trusted LAN. `fleet-daemon pair` writes only the token file; it
does not edit the daemon TOML.

### Known limitations of the current implementation

- Same-host / same-LAN only: the daemon must reach the admin HTTP API and the
  machine WebSocket directly. Remote enrollment through a tunnel works only if
  the operator supplies the same reachable URL and a code out of band.
- No QR code, OAuth-style device flow, browser approval screen or UI pairing.
- No deferred/offline enrollment: a code is redeemed immediately and expires
  after `auth.pair_code_ttl_secs` (default 600 seconds).
- The upgrade limiter is in-memory and per source IP, and it only charges
  unauthenticated (pairing) upgrades; authenticated token reconnects are not
  throttled. It resets on admin restart and is not shared between admin
  instances. This is separate from the durable per-code attempt limit, which is
  stored with the code.
- Re-pairing with a different key requires the old key's rotation proof. Losing
  the old key for a pinned machine cannot be fixed by "revoke + unrevoke",
  because unrevocation restores the recorded pinned key. Rotate the key while
  the old key is still available (`agent-daemon rotate-key`), or have an
  administrator clear the pinned key out of band before a fresh key can pair.
  Revocation destroys the old token, so it never becomes valid again; a
  re-pair is required.
- A revoked machine cannot re-pair until it is explicitly unrevoked.

What this does **not** solve: network reachability. Pairing is orthogonal to how
packets travel between daemon and admin; see sections 3 and 4.

---

## 2. Pairing code details

A **pairing code** is a short-lived, single-use secret used only to bootstrap
trust. It is not a long-lived credential.

### Format and storage

- 12 Crockford Base32 symbols rendered as `XXXX-XXXX-XXXX` (no I, L, O or U;
  input is case-insensitive and accepts `I`/`L` as `1`, `O` as `0`).
- Bound to an organization, the creating principal and an optional machine-name
  hint. The organization must exist.
- TTL `auth.pair_code_ttl_secs` (default 600 seconds); at most
  `auth.pair_code_max_attempts` (default 5) failed handshakes per code.
- Only `SHA-256(normalized code)` is stored. The plaintext is returned once and
  redacted from `Debug` output and logs.
- Redemption is a single SQLite transaction: the code is marked used and the
  machine key/token are recorded together, so the same code cannot pair two
  machines.

### Lifecycle

1. `POST /api/v1/pairing/codes` (`af machines pair-code`) creates the code and
   appends a `pairing.code.create` audit entry.
2. The daemon sends `connection.hello` with `pairing_code` (no
   `Authorization` header), receives the challenge and signs it.
3. The admin validates the code, verifies the signature, and checks the rotation
   proof if the machine id already has a different pinned key. Only a bad
   challenge signature records a failed attempt and a `machine.pair_denied`
   audit row; a missing or invalid rotation proof is rejected with
   `machine_key_mismatch` and consumes neither an attempt nor the code.
4. Only after all checks pass does the transaction consume the code, pin the
   key, record the machine/org and return `connection.accepted` with
   `machine_token` and `org_id`.
5. The daemon saves the token to `<key_file>.token` (0600) and drops the code.

### Security properties

- ~60 bits of entropy; codes are random, never sequential.
- Hash-only at rest; the plaintext is never written to daemon config.
- Strict TTL, single use, attempt limiting and audit records.
- The Ed25519 challenge/response is kept. The code authorizes only the initial
  enrollment; it never replaces possession of the key.
- The code is bound to an organization; a machine id from another organization
  is rejected with `machine_org_mismatch`.
- Invalid, expired, used and locked codes are indistinguishable
  (`pairing_code_invalid`) to prevent enumeration.

### Future work

- Remote/offline/deferred enrollment and a browser approval screen.
- QR bootstrap that encodes `server_url`, a one-shot code and an org hint for
  headless machines.
- OAuth device flow for SSO-backed fleets.
- Per-IP throttling (the current limiter is per process, so NAT peers share a
  budget) and a durable attempt ledger across restarts.
- Optional mTLS enrollment for private-PKI deployments.

---

## 3. Why the reverse WebSocket is still the right base

The current design has the daemon open a long-lived outbound WSS connection to
the admin. That choice is correct and should be preserved:

- Works when the daemon is behind NAT, CGNAT, a home router or a corporate
  firewall that blocks inbound connections.
- One connection carries requests, responses and events in both directions.
- Reconnect with exponential backoff and heartbeat ACKs is simple and robust.
- The admin keeps a single authoritative connection registry.
- No inbound port on developer machines.

The failure mode people hit is usually **not WebSocket itself**, but **the admin
not being reachable** from the daemon network: private IP, missing DNS, blocked
443, TLS interception, or an idle proxy dropping the connection.

WebSocket is widely supported on 443. Corporate proxies that block HTTP
`Upgrade` or non-443 ports are the exception, not the rule; plan a fallback for
them rather than abandoning the design.

---

## 3.1 Machine ↔ admin channel options

The browser is out of scope here: it always talks to the admin over REST/WSS.
This table compares only how a **daemon** reaches the **admin**.

| Option | NAT traversal | Bidirectional | Proxy tolerance | Ops cost | Verdict |
|---|---|---|---|---|---|
| **Reverse WSS to a reachable admin** | Daemon outbound only | Yes | Good on 443; some proxies break Upgrade | Low | Default |
| **Reverse WSS through a private overlay** (Tailscale/WireGuard) | Both sides join overlay | Yes | Overlay handles it | Medium | Best for private fleets |
| **Reverse WSS through an admin-side tunnel** (Cloudflare Tunnel, frp) | Daemon outbound to public host | Yes | Good on 443 | Medium | Best when admin cannot accept inbound |
| **Site relay / gateway** | Relay outbound to admin; daemons local | Yes (multiplexed) | Good | High | Required for isolated LANs that cannot reach the admin |
| **Message broker** (MQTT/NATS) | Both sides outbound to broker | Yes (topics) | Good on 443/TLS | High | Large fleets, strict egress, existing broker |
| **gRPC bidirectional streaming** | Same as WSS | Yes | Poor through HTTP/1.1 proxies | Medium | Only when HTTP/2 is guaranteed end-to-end |
| **SSH / reverse SSH tunnel** | Requires an SSH jump host | Yes | Depends on SSH egress | Low but manual | Break-glass, not product |
| **WebRTC data channel** | STUN/TURN | Yes | TURN needed | Very high | Chat/media, not a control channel |
| **Long polling** | Outbound HTTP | Request/response plus poll | High | Low | Last-resort fallback when Upgrade is blocked |

Recommendation: **reverse WSS to a stable reachable endpoint** remains the best
default. Use an overlay or an admin-side tunnel when the admin itself is not
publicly reachable. Add a relay only when machines cannot reach any common
endpoint, and add long polling only after a concrete proxy-blocking case.

### Where the admin lives

- Public DNS + reverse proxy on 443 (self-hosted, VPS, Kubernetes ingress).
- Private overlay address (Tailscale MagicDNS, WireGuard IP).
- Outbound tunnel from the admin host (Cloudflare Tunnel, frp client).
- A relay VPS when the admin has no public ingress and no overlay.
- **Do not** require inbound ports on developer machines.

If both admin and daemons are behind NAT and no overlay is allowed, a relay is
mandatory; NAT hole punching is not reliable enough for a control channel.

---

## 4. Deployment topologies

| Topology | When to use | Pros | Cons |
|---|---|---|---|
| **Direct LAN** | Admin and daemons on one trusted network | Simplest, lowest latency | No WAN access; LAN DNS/TLS must be managed |
| **Private overlay** (Tailscale / WireGuard) | Small/medium teams, remote machines, no public exposure | Private, automatic TLS with Tailscale Serve, no inbound ports on admin | Requires joining a tailnet/VPN; client installation and ACLs |
| **Tunnel** (Cloudflare Tunnel, frp, ngrok) | Admin cannot accept inbound traffic but needs public/remote access | Admin remains loopback-only; no port forwarding; TLS at edge | Public surface unless Access/WAF is added; long-lived WS limits; vendor dependency |
| **Public reverse proxy** | Self-hosted admin with a stable domain | Standard TLS, HSTS, mTLS, rate limits, your own DNS | Requires public ingress, certificates, firewall and DDoS protection |
| **Self-hosted relay** | Large fleets, both sides behind NAT, strict egress | Daemons/administrators only need outbound 443 | Relay sees traffic unless end-to-end encryption/mTLS; extra service to operate |
| **Air-gapped LAN** | Regulated/offline environments | No external dependency | Manual updates and certificates |

### Multi-LAN / segmented networks

Two patterns work:

1. **Common reachable admin (preferred).** Admin on a VPS, tailnet or tunnel;
   every daemon connects outbound to it. Multiple LANs are irrelevant because
   the connection is initiated from each daemon.
2. **Site relay per isolated LAN.** When daemons cannot reach the admin (for
   example, an air-gapped plant network), run a small relay inside that LAN:

   ```
   daemon A ─┐
   daemon B ─┼─ LAN mTLS ─▶ site relay ── outbound WSS ──▶ admin
   daemon C ─┘
   ```

   The relay connects outbound to the admin, authenticates local daemons with
   mTLS or pairing codes, multiplexes their connections, and never accepts
   inbound traffic from the internet. The admin treats each daemon as a normal
   machine; the relay is a transport hop, not a second control plane.

This pattern is the practical answer for “different machines are in different
LANs and cannot all reach one another”. It scales better than deploying a
separate admin per site and avoids inbound firewall changes.

### Recommendation by scale

- **Local development**: `ws://127.0.0.1:8787` (current default).
- **Small team, trusted operators**: Tailscale Serve on the admin host; daemons
  join the tailnet and use `wss://<admin>.<tailnet>.ts.net/ws/machine`. Keep the
  admin bound to loopback.
- **Remote machines without a VPN**: Cloudflare Tunnel to the admin's loopback
  port, with Cloudflare Access in front of the UI. Daemons connect to the public
  WSS hostname; each daemon's per-machine token still authenticates it.
- **Enterprise**: reverse proxy on 443 with mTLS or strict token policy, DNS SRV
  for multiple endpoints, egress proxy support, and audit of proxy headers.
- **Both sides behind NAT with no overlay**: a relay is required; do not attempt
  hole punching as the primary mechanism.

---

## 5. Endpoint configuration and discovery

Configuration priority (highest first):

1. CLI flag
2. environment variable
3. daemon TOML
4. discovered endpoint (LAN/QR bootstrap)
5. default `ws://127.0.0.1:8787`

Recommended additions:

- **Multiple endpoints**: `server_urls = ["wss://a", "wss://b"]` with ordered
  failover. Try each on connect/reconnect, keep the current one until the
  connection fails twice, and expose the active endpoint in the UI.
- **DNS SRV / well-known**: `_agent-fleet._tcp.example.com` SRV records or
  `GET /.well-known/agent-fleet` returning candidate URLs. This lets operators
  move the admin without reconfiguring every daemon.
- **LAN discovery (opt-in)**: mDNS service advertisement
  (`_agent-fleet._tcp`) for `admin.local`, only on trusted networks. A discovered
  endpoint must still pass token and Ed25519 verification; discovery is never trust.
- **QR / bootstrap URI**: encode `server_url`, pairing code and org hint in a QR
  code for headless machines. The code is single-use and short-lived.

Do not silently fall back to plaintext HTTP on non-loopback interfaces.

---

## 6. Connection lifecycle and robustness

The current client already has jittered exponential backoff and heartbeats.
Add:

- **Ping/pong at the WebSocket layer** in addition to application heartbeats;
  many proxies drop idle connections after 60 seconds.
- **Server ACK for heartbeats** (already present) and connection deadlines.
- **Resumable event streams**: daemon events already carry a per-connection
  sequence. Add a resume cursor in `connection.hello` so the admin can request a
  replay of missed events or ask the daemon for a full snapshot.
- **Bounded offline queue**: queue non-high-frequency events while disconnected;
  cap it by count/bytes; drop or coalesce deltas, never permissions/terminal
  state.
- **Reconnect jitter** with a cap and a minimum, plus a "fast retry" budget for
  transient Wi-Fi/tunnel changes.
- **Connection health metrics**: attempt count, last error, active endpoint,
  round-trip heartbeat latency, last successful event sequence. Expose them in
  the admin UI and logs.
- **Clock skew tolerance**: use monotonic durations for reconnect and timeouts;
  do not compare wall clocks across machines for ordering.
- **Egress proxies**: honor `HTTPS_PROXY` / `NO_PROXY` with HTTP CONNECT for the
  WebSocket upgrade, and allow a custom CA bundle for corporate TLS inspection.
  This is a common enterprise requirement and currently a gap.

---

## 7. If WebSocket is blocked

First verify what is actually blocked (DNS, TCP 443, TLS interception, or HTTP
Upgrade). Mitigations, in order of preference:

1. **WSS on 443 through a reverse proxy or CDN** that supports WebSocket.
2. **Private overlay** (Tailscale/WireGuard) if public ingress is not allowed.
3. **Outbound-only relay** on 443 that forwards framed messages. The relay should
   not terminate application credentials; use TLS/mTLS or end-to-end payload
   encryption.
4. **HTTP/2 or gRPC bidirectional streaming** where available end-to-end.
5. **Long polling** as a last resort: `POST` outbound requests plus periodic
   event fetch. It is less efficient and not a replacement for streaming UX,
   but it can traverse proxies that reject upgrades.
6. **SSE is not sufficient alone** for the daemon link: SSE is server→client
   only, so it needs a separate request channel and cannot carry correlated
   bidirectional RPC as cleanly.

Keep WebSocket as the primary transport and add fallbacks behind the same
transport abstraction only when a real deployment requires them.

---

## 8. Security checklist

- TLS/WSS everywhere outside loopback; validate certificates; allow a custom CA
  bundle for private PKI.
- Optional mTLS for admin↔daemon; bind tokens to the machine key so a stolen
  token cannot replace the key silently.
- Pair codes: TTL, single use, hashed at rest, attempt limited, audited (all
  implemented for the same-host/same-LAN path).
- Per-machine tokens with revocation (implemented). There is no shared token,
  no key-only mode and no fallback to the user/admin token.
- Proxy awareness: trust `X-Forwarded-*` only from configured proxies; enforce
  HTTPS/HSTS; rate-limit authentication endpoints.
- Keep the daemon outbound-only. If an inbound listener is unavoidable, bind it
  to loopback and tunnel it.
- Do not log tokens, pairing codes or full Authorization headers. Redact query
  strings in access logs.

---

## 8.1 What not to do

- Do not require inbound ports on daemons or developer machines.
- Do not expose the admin directly on a public interface without TLS and rate
  limits; prefer loopback + overlay/tunnel/reverse proxy.
- Do not rely on NAT hole punching as the primary control-channel mechanism.
- Do not reintroduce a single global machine token or a key-only/TOFU path;
  enroll with a code and use the per-machine token.
- Do not silently fall back to plaintext HTTP outside loopback.
- Do not assume SSE can replace the duplex channel; it is server→client only.
- Do not run two admins against one daemon without leases/fencing.

## 9. Proposed work items

1. **Remote/offline pairing**: QR bootstrap, OAuth device flow, UI approval and
   deferred enrollment. Same-host/same-LAN pairing plus per-machine tokens is
   implemented.
2. **Multiple admin endpoints with failover** (`server_urls`, connection health,
   UI display).
3. **Resumable event streams** (resume cursor, bounded queue, snapshot resync).
4. **DNS SRV / well-known discovery** and opt-in mDNS.
5. **Transport abstraction and relay/fallback** only after a concrete blocked-WS
   deployment case.
6. **mTLS option** for private PKI deployments.
7. **HTTPS proxy + custom CA support** for the daemon WebSocket client.
8. **Accurate per-IP throttling behind a trusted proxy**, plus a durable
   upgrade-attempt ledger across restarts (the in-memory limiter resets on
   restart).
9. **Site relay mode** for isolated LANs (multiplexed outbound connections).
10. **Connection leases / HA** if more than one admin instance is deployed, so
    two admins never drive the same daemon at once.
