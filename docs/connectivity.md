# Connectivity and Machine Enrollment

This document covers two related problems:

1. **How a daemon joins an admin safely** — pairing/enrollment codes versus the
   current shared machine token.
2. **How a daemon reaches the admin across LAN, NAT and the public internet** —
   why the reverse WebSocket design is still right, and which transport,
   discovery and fallback strategies fit which deployment.

The guiding principle is: **the daemon only makes outbound connections**. It
never listens for admin traffic. This keeps daemons usable behind NAT,
corporate firewalls and home routers, and keeps their attack surface small.

---

## 1. Current machine authentication

Today every daemon connects with:

- a shared `AGENT_FLEET_MACHINE_TOKEN` in the WebSocket `Authorization` header;
- an Ed25519 key pair (`~/.agent-fleet/machine.key`);
- a challenge signed by that key;
- trust-on-first-use: the admin pins the first public key it sees per machine id;
- rotation proof signed by the previously pinned key.

This is secure against impersonation **after** pairing, but it has onboarding
weaknesses:

- one leaked global token is an entry point for every admin;
- each new machine needs the shared token distributed out of band;
- a token leak during first contact lets an attacker pin their own key;
- there is no explicit "user X enrolled machine Y into org Z" record;
- revocation is recorded per machine id, but the credential itself is still
  shared: there is no per-machine long-lived token yet.

**Current revocation behavior.** `DELETE /api/v1/machines/{id}` now writes a
permanent `machine_revocations` record, disconnects the live connection and
removes the active machine row. A revoked daemon is rejected during the
handshake and stops reconnecting instead of backing off indefinitely.
`GET /api/v1/machines/revoked` lists revocations and
`POST /api/v1/machines/{id}/unrevoke` (or `af machines unrevoke <id>`) allows
the machine to pair again. This closes the "delete cannot revoke" gap but does
not replace per-machine enrollment credentials or pairing codes.

---

## 2. Pairing / enrollment codes

A **pairing code** (also called an enrollment code) is a short-lived, single-use
secret used only to bootstrap trust. It is not a long-lived credential.

### Flow

1. An org admin (UI or `af`) creates an enrollment:
   - random high-entropy code, e.g. 10 characters from a 32-symbol alphabet;
   - bound to an organization, creator, optional machine name;
   - TTL 5–10 minutes, single use, limited attempts;
   - only a hash of the code is stored.
2. The daemon starts with the code:

   ```bash
   fleet-daemon pair \
     --server wss://fleet.example.com/ws/machine \
     --code <enrollment-code>
   ```

   Alternatively the code is placed in `[connection] pairing_code` or an env
   var for automated provisioning.
3. The daemon sends `connection.hello` with its machine id, public key and the
   code.
4. The admin:
   - validates the code (exists, unused, unexpired, attempts left);
   - verifies the Ed25519 challenge signature as today;
   - pins the public key;
   - marks the enrollment used and records the machine/org/actor;
   - returns a **per-machine long-lived token** (only its hash is stored).
5. The daemon stores the token next to its key file with mode 0600 and uses it
   for every subsequent connection. The code cannot be replayed.
6. The global machine token can be disabled, or kept as a compatibility
   fallback during migration.

### Security requirements

- At least ~50 bits of entropy; never sequential or guessable.
- Store `hash(code)`, never the plaintext.
- Strict TTL, single use, attempt rate limiting and per-IP throttling.
- Bind the enrollment to an organization and a creator; audit the redemption.
- Keep the Ed25519 challenge/response. The code only authorizes the *first*
  pin; it never replaces possession-of-key.
- Reject a second machine id using the same enrollment.
- Deliver codes over TLS/WSS or an authenticated UI; do not put them in shell
  history, logs or URLs that get indexed.
- Rate-limit failed enrollments and surface them in the audit log.

### What it does and does not solve

- Solves: shared-secret distribution, per-machine revocation, onboarding audit,
  self-service installation.
- Does not solve: network reachability. Pairing is orthogonal to how packets
  travel between daemon and admin.

### Alternatives

- **Per-machine tokens without codes**: admin generates a token per machine and
  the operator copies it once. Simpler, but still manual and easy to leak.
- **OAuth device flow**: the daemon shows a URL and code, a user approves in the
  browser, and the daemon polls for a token. Best UX and integrates with SSO,
  but requires more state and browser flows.
- **mTLS enrollment**: a CA issues a client certificate during provisioning.
  Strong, but certificate distribution is operationally heavier.

Recommendation: pair codes + per-machine tokens are the best next step for this
project; OAuth device flow can layer on later.

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
  WSS hostname; the machine token still authenticates them.
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
- Pair codes: TTL, single use, hashed at rest, rate limited, audited.
- Per-machine enrollment tokens with revocation. The shared machine token is
  required today and never falls back to the user/admin token; per-machine
  enrollment tokens remain on the roadmap.
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
- Do not use a single global machine token as the long-term design; move to
  per-machine enrollment credentials.
- Do not silently fall back to plaintext HTTP outside loopback.
- Do not assume SSE can replace the duplex channel; it is server→client only.
- Do not run two admins against one daemon without leases/fencing.

## 9. Proposed work items

1. **Pairing codes + per-machine tokens** (schema, admin API/UI/`af`, daemon
   `pair` command, audit, rate limits).
2. **Multiple admin endpoints with failover** (`server_urls`, connection health,
   UI display).
3. **Resumable event streams** (resume cursor, bounded queue, snapshot resync).
4. **DNS SRV / well-known discovery** and opt-in mDNS.
5. **Transport abstraction and relay/fallback** only after a concrete blocked-WS
   deployment case.
6. **mTLS option** for private PKI deployments.
7. **HTTPS proxy + custom CA support** for the daemon WebSocket client.
8. **Site relay mode** for isolated LANs (multiplexed outbound connections).
9. **Connection leases / HA** if more than one admin instance is deployed, so
   two admins never drive the same daemon at once.
