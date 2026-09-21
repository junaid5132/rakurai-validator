# Setup Guide

**This is how you join TIN.** Onboarding registers your block engine on-chain so every opted-in Rakurai validator connects to it automatically, and your orderflow starts competing for inclusion alongside the other landing services.

You hand the Rakurai team two things — a **discovery endpoint** that lists your block engine URLs, and a **wallet pubkey you control** for PSA / MCA settlement — and Rakurai performs the on-chain registration. This page covers that handover and the gRPC services your server must expose.

**Audience:** Block engines, searchers, and partners integrating transaction landing with Rakurai.

**Sample server:** [tin_sample_servers](https://github.com/rakurai-io/tin_sample_servers) is a minimal working reference:

- `bundles_server` — Auth + Validator / discovery + dummy tip bundles
- `p2c_server` — Auth + Relayer / P2C logging + `GetBlockEngineEndpoints`

Use it to see the required gRPC services and flags; replace the dummy logic with your own. Run details: [§7](#7-sample-server).

> [!NOTE]
> **The connection is outbound from the validator**
>
> The validator **connects to the URL you provide**, while you run a gRPC **server** at that address. Make sure the server is bound to an address that validators can reach. Both `http://` and `https://` URLs are supported.

Protos: [`auth.proto`](../../p2c-protos/protos/auth.proto), [`block_engine.proto`](../../p2c-protos/protos/block_engine.proto), [`packet.proto`](../../p2c-protos/protos/packet.proto). Keep message and RPC names — renaming them breaks the validator.

---

## 1. Contact Rakurai and share your wallet pubkey

On [Discord](https://discord.gg/XS7GmnmCJg) or [Telegram](https://t.me/rakurai_official), share:

1. Your **global / discovery URL**(s) — one for the **block engine (bundles)** path, and one for **P2C** (same URL is fine if one host serves both). Put **all regional endpoints behind** each discovery URL via `GetBlockEngineEndpoints` (`global_endpoint` + `regioned_endpoints`). Only the discovery URL is registered on-chain; regions are not shared with Rakurai one-by-one.
2. A **wallet pubkey you control** (you hold the private key) — used for **PSA / MCA** recording and settlement

Rakurai registers the discovery URL(s) and creates per-validator PSA / MCA (and tip / TCA as needed). Keep that key secure for epoch settlement ([`rakurai-revshare`](../rakurai_programs/cli/partner_reward_settlement.md)). Details: [§2](#2-discovery--global-url-for-bundles-and-p2c).

---

## 2. Discovery — global URL for bundles and P2C

Both paths use the **same** discovery RPC: `BlockEngineValidator.GetBlockEngineEndpoints`. Share one **global / discovery URL** with Rakurai per path (or one URL if a single host serves both). Validators call that URL, then connect to the lowest-latency `block_engine_url` you return (same idea as Jito).

| Path | What you share with Rakurai | What you put behind it |
|------|-----------------------------|------------------------|
| **Block engine (bundles)** | Discovery URL | Regional BE hosts in `regioned_endpoints` (each serves Auth + subscribe streams) |
| **P2C (post-pack)** | Discovery / Relayer URL | Regional P2C hosts in `regioned_endpoints` (each serves Auth + Relayer streams) |

### 2.1. Discovery vs regioned endpoints

| Piece | Who sets it | Notes |
|-------|-------------|--------|
| **Discovery URL** | You share once; Rakurai registers it on-chain | e.g. `http://api.example.com:2345` |
| **`global_endpoint` / `regioned_endpoints`** | You return them from `GetBlockEngineEndpoints` | Not registered per-region; change by redeploying discovery |

Flow:

1. Run Auth + `BlockEngineValidator` (including `GetBlockEngineEndpoints`) on your discovery host.
2. Share that discovery URL with Rakurai — only this URL goes on-chain.
3. Validators dial it → `GetBlockEngineEndpoints` → your `global_endpoint` and `regioned_endpoints`.
4. They probe regions by latency, then reconnect to the best reachable URL (fallback: `global_endpoint`, or the seed URL if discovery fails).

To add a region later: deploy the same gRPC stack there, then add that URL to `regioned_endpoints` on the discovery server. No second on-chain registration. Edit the list in your `GetBlockEngineEndpoints` handler (see [`bundles_server`](https://github.com/rakurai-io/tin_sample_servers/tree/development/bundles_server) / [`p2c_server`](https://github.com/rakurai-io/tin_sample_servers/tree/development/p2c_server)) — not client-config CLI or Admin RPC.

**P2C uses the same discovery RPC.** Put `GetBlockEngineEndpoints` on the P2C process; `SubscribePackets` / `SubscribeBundles` can stay `UNIMPLEMENTED` if you do not serve bundles there.

### 2.2. `GetBlockEngineEndpoints` request / response

**Request** (empty):

```protobuf
GetBlockEngineEndpointRequest {}
```

**Response:**

```protobuf
GetBlockEngineEndpointResponse {
  global_endpoint {
    block_engine_url: "https://gateway.your-provider.com"
    shredstream_receiver_address: ""
  }
  regioned_endpoints {
    block_engine_url: "https://fra.gateway.your-provider.com"
    shredstream_receiver_address: ""
  }
  regioned_endpoints {
    block_engine_url: "https://nyc.gateway.your-provider.com"
    shredstream_receiver_address: ""
  }
}
```

| Field | Required? | Meaning |
|-------|-----------|---------|
| `global_endpoint` | Optional | Fallback / single-host URL. Omitted is fine if `regioned_endpoints` is non-empty. |
| `regioned_endpoints` | Recommended for multi-region | Every regional URL validators may dial. Probe + pick lowest RTT. |
| `block_engine_url` | Yes (per endpoint) | Reachable `http://` or `https://` URL (not `127.0.0.1` / `0.0.0.0` / `localhost`). |
| `shredstream_receiver_address` | Optional | Unused for most TIN partners; may be `""`. |

`global_endpoint` alone is enough for a single host. With multiple regions, list every regional URL in `regioned_endpoints`. The sample echoes `--public-url` into the response — replace that list when you multi-home.

> [!WARNING]
> **`block_engine_url` must be reachable from the validator**
>
> Do not return `http://127.0.0.1:…`, `http://0.0.0.0:…`, or `localhost`. Those only work on the same machine. Use a public hostname or IP (and port) that remote validators can dial — e.g. `http://203.0.113.10:10000` or `https://be.example.com`. Listen with bind `0.0.0.0`; that is separate from the URL you advertise.

Quick check (no server reflection required):

```bash
cd p2c-protos/protos
grpcurl -plaintext -import-path . -proto block_engine.proto \
  <DISCOVERY_HOST>:<PORT> \
  block_engine.BlockEngineValidator/GetBlockEngineEndpoints
```

---

## 3. Auth (shared by both paths)

Both bundles and P2C use `auth.AuthService`. Role differs by path.

| Path | Role |
|------|------|
| **Bundles** (`BlockEngineValidator` streams) | `VALIDATOR` |
| **P2C** (`BlockEngineRelayer` streams) | `RELAYER` |

Flow:

1. `GenerateAuthChallenge` — `{ role, pubkey }` → `{ challenge }`
2. Client signs `"{pubkey}-{challenge}"`, then `GenerateAuthTokens` → `{ access_token, refresh_token }`
3. Later RPCs: `authorization: Bearer <access_token.value>`

```protobuf
GenerateAuthChallengeRequest { role: VALIDATOR, pubkey: <32 bytes> }
GenerateAuthChallengeResponse { challenge: "…" }

GenerateAuthTokensRequest {
  challenge: "<pubkey>-<challenge>"
  client_pubkey: <32 bytes>
  signed_challenge: <64-byte sig>
}
GenerateAuthTokensResponse {
  access_token { value: "…" expires_at_utc { … } }
  refresh_token { value: "…" expires_at_utc { … } }
}
```

Reject identities that are not allowed to connect (e.g. not on the leader schedule / not a Rakurai client).

---

## 4. Set up `BlockEngineValidator` (bundles)

After `VALIDATOR` auth, the validator opens subscribe streams and **you push** packets / bundles for the life of the connection.

| RPC | Required? | Direction |
|-----|-----------|-----------|
| `GetBlockEngineEndpoints` | **Yes** (on discovery host) | Validator → you (unary) |
| `SubscribePackets` | **Yes** for bundles | You → validator (server stream) |
| `SubscribeBundles` | **Yes** for bundles | You → validator (server stream) |
| `GetBlockBuilderFeeInfo` | Optional | Unary fee info |

### 4.1. Request / response schemas

```protobuf
# Request (empty) + Bearer
SubscribePacketsRequest {}
SubscribeBundlesRequest {}

# Stream responses
SubscribePacketsResponse {
  header { ts { … } }
  batch { packets { data: <wire tx> meta { size: … } } }
}

SubscribeBundlesResponse {
  bundles {
    uuid: "…"
    bundle { packets { data: <wire tx> … } }
  }
}
```

Keep streams open for the connection lifetime. Once connected, send bundles with a tip to a Rakurai tip account.

> [!TIP]
> **Recommended tip**
>
> **1,000,000 lamports (0.001 SOL)** per tipped transaction or bundle (in addition to normal priority fees). Tip accounts: [Tips](./tips.md).

---

## 5. Set up `BlockEngineRelayer` (P2C)

After `RELAYER` auth, the validator opens **one TLS/channel** and up to **three** bi-di streams. Identify traffic by **which RPC** you accepted — the wire `PacketBatchUpdate` shape is the same for scheduler and TPU.

Also expose `GetBlockEngineEndpoints` on `BlockEngineValidator` on the same host (or a dedicated discovery URL) so P2C autoconfig can rank regions — same request/response as [§2.2](#22-getblockengineendpoints-request--response).

| RPC | Required? | Default on sample | Content |
|-----|-----------|-------------------|---------|
| `StartExpiringPacketStream` | **Yes** | always on | Scheduler / post-pack txs (point of no return) |
| `StartExpiringTpuPacketStream` | Optional | on (`--disable-tpu-packet-stream` to reject) | TPU / sigverify packets |
| `StartP2cUpdateCountStream` | Optional | on (`--disable-p2c-update-count` to reject) | Per-slot send counts |

Optional RPCs may return `UNIMPLEMENTED`. Newer validators keep the scheduler stream and skip the missing ones; they do **not** tear down the whole connection.

### 5.1. Differentiating Scheduler vs TPU

The two streams represent different points in the transaction processing pipeline. The stream used depends on whether the validator is currently the leader or not.

| Stream | When | Meaning |
|--------|------|---------|
| `StartExpiringPacketStream` | **When the validator is the leader** | **Post-pack path** — the transaction has reached the point of no return for the current leader slot, so it is streamed to P2C. |
| `StartExpiringTpuPacketStream` | **When the validator is not the leader** | **TPU path** — when the validator receives a transaction packet, it forwards the packet to P2C. This does **not** guarantee that the transaction will be included ahead of other transactions. |

> **Note:** Do **not** rely on a packet field named `source` to differentiate these paths. The RPC stream itself identifies whether the packet came through the scheduler or TPU path.

**Secondary (via `meta`):** both paths fill `Meta`, but `addr` differs:

| Field | Scheduler stream | TPU stream |
|-------|------------------|------------|
| `meta.size` | `data.len()` | `data.len()` |
| `meta.addr` | Per-tx string from the scheduler (not an IP) | **Validator identity signature** over the transaction-signature string (proof this leader emitted the update) |

Treat `meta.addr` as opaque proof / correlation data — never parse it as a socket address. Full packet layout: [Using P2C — meta fields](./post_pack/using_p2c.md#22-meta-fields).

### 5.2. Slot on the wire (`expiry_ms`) — what and why

Each `ExpiringPacketBatch` carries:

```protobuf
expiry_ms: <working-bank slot as u32>
```

**What it is:** The validator **working-bank slot** when the update was sent.

**Why:** so you can group, debounce, and time replies against the leader slot that produced the update — without a separate slot field on every packet. When the slot rolls, treat it as a new window (same idea as the count stream below).

### 5.3. `P2cUpdateCount` — what and why

When `StartP2cUpdateCountStream` is enabled, the validator pushes one `P2cUpdateCount` **per slot** (and flushes the in-progress slot on disconnect):

| Field | Meaning |
|-------|---------|
| `uuid` | Post-pack endpoint UUID |
| `slot` | Slot these counts belong to |
| `scheduler_count` | Successful sends on `StartExpiringPacketStream` that slot |
| `tpu_count` | Successful sends on `StartExpiringTpuPacketStream` that slot |
| `total_count` | `scheduler_count + tpu_count` |
| `p2c_tpu_enabled` | Whether this endpoint has TPU updates enabled in config |

**Why:** health / volume signal without counting packets yourself — confirm the leader is connected, how much scheduler vs TPU traffic you saw that slot, and catch silent drops (heartbeats without counts).

### 5.4. Packet / count schemas

```protobuf
# Validator → you (scheduler or TPU stream)
PacketBatchUpdate {
  batches {
    batch { packets { data: <wire tx> meta { size: … addr: "<p2c string>" port: 0 } } }
    # expiry_ms carries the working-bank slot as u32 (not a millisecond expiry)
    expiry_ms: <slot_u32>
  }
}

# Validator → you (count stream)
P2cUpdateCount {
  uuid: "…"
  slot: …
  scheduler_count: …
  tpu_count: …
  total_count: …
  p2c_tpu_enabled: true|false
}

# You → validator (heartbeats on each open Relayer stream)
StartExpiringPacketStreamResponse { heartbeat { count: 1 } }
```

Packet decoding and reply bundles: [Using P2C](./post_pack/using_p2c.md).

---

## 6. Same URL for bundles and P2C

One registered URL must expose:

| Service | What the validator uses |
|---------|-------------------------|
| `auth.AuthService` | `VALIDATOR` **and** `RELAYER` |
| `block_engine.BlockEngineValidator` | `GetBlockEngineEndpoints`, `SubscribePackets` / `SubscribeBundles` |
| `block_engine.BlockEngineRelayer` | `StartExpiringPacketStream` (**required**); TPU + count streams optional |

> [!WARNING]
> **Same URL without the Relayer role**
>
> Bundles can work while P2C fails. Validator log: `SchedulerUpdateNotifier: connection to {url} failed`.

Separate URLs are fine (Validator-only vs Relayer-only hosts). Relayer-only hosts still need `GetBlockEngineEndpoints` for P2C region autoconfig.

---

## 7. Sample server

[tin_sample_servers](https://github.com/rakurai-io/tin_sample_servers) is a working reference: `bundles_server` (Auth + Validator / discovery) and `p2c_server` (Auth + Relayer + `GetBlockEngineEndpoints`, with TPU and count streams on by default).

```bash
cargo build -p bundles_server --release
RUST_LOG=info ./target/release/bundles_server \
  --bind 0.0.0.0:10000 \
  --public-url http://<PUBLIC_HOST>:10000 \
  --allow-any-validator
```

```bash
cargo build -p p2c_server --release
RUST_LOG=info ./target/release/p2c_server \
  --bind 0.0.0.0:10001 \
  --public-url http://<PUBLIC_HOST>:10001
# optional: --disable-tpu-packet-stream --disable-p2c-update-count
```

`--bind` is the listen address (`0.0.0.0` is fine). `--public-url` is what `GetBlockEngineEndpoints` returns — use a host validators can reach (not `127.0.0.1` / `0.0.0.0`). Share that discovery URL with Rakurai. Auth is limited to pubkeys on `getLeaderSchedule` that advertise the Rakurai client id in `getClusterNodes`.

---

## 8. For validators — get endpoints (bundles and P2C)

Admin IPC is request/response: keep the socket open briefly so `socat` can read the reply.

Use these to confirm what the node will dial after on-chain registration + discovery:

| Path | Admin RPC | What it shows |
|------|-----------|---------------|
| **Block engine (bundles)** | `getBlockEngineUrls` | Primary + secondary BE URLs, on-chain merge, blocklist |
| **P2C (post-pack)** | `getPostPackConfirmationConfig` | Active P2C URLs / UUIDs, TPU flag, blocklist |

### 8.1. Block engine — `getBlockEngineUrls`

```bash
(echo '{"jsonrpc":"2.0","id":1,"method":"getBlockEngineUrls","params":[]}'; sleep 1) \
  | socat - UNIX-CONNECT:admin.rpc | jq
```

```json
{
    "active_secondary_entries": [],
    "admin_secondary_entries": [],
    "blocklisted_uuids": [],
    "onchain_secondary_entries": [],
    "primary_url": "https://frankfurt.mainnet.block-engine.jito.wtf"
}
```

**How the validator uses it:** `primary_url` (and active secondaries) are the seed / configured BE hosts. The node calls `GetBlockEngineEndpoints` on those URLs, probes `regioned_endpoints`, and connects to the best reachable region for bundle subscribe streams.

### 8.2. P2C — `getPostPackConfirmationConfig`

```bash
(echo '{"jsonrpc":"2.0","id":1,"method":"getPostPackConfirmationConfig","params":[]}'; sleep 1) \
  | socat - UNIX-CONNECT:admin.rpc | jq
```

| Field | Description |
|-------|-------------|
| `onchain_entries` | Entries from the on-chain PDA (`url`, `uuid`, `enable_tpu_p2c_update`) |
| `admin_entries` | Admin-set entries (same shape) |
| `blocklisted_uuids` | UUIDs blocked via Admin RPC |
| `active_entries` | Merged admin + on-chain, excluding blocklist — endpoints that receive Relayer streams |

**How the validator uses it:** for each `active_entries[].url`, the node authenticates as `RELAYER`, calls `GetBlockEngineEndpoints` when available, probes regions, then opens `StartExpiringPacketStream` (and optionally TPU + count streams). Full field examples: [Using P2C — Commands](./post_pack/using_p2c.md#4-commands).

### 8.3. Blocklists

**Block engine:**

```bash
# Block one UUID
(echo '{"jsonrpc":"2.0","id":1,"method":"setBlockEngineUrlBlocklist","params":[["<BlockEngine1>"]]}'; sleep 1) \
  | socat - UNIX-CONNECT:admin.rpc

# Clear — must be params:[[]] not params:[]
(echo '{"jsonrpc":"2.0","id":1,"method":"setBlockEngineUrlBlocklist","params":[[]]}'; sleep 1) \
  | socat - UNIX-CONNECT:admin.rpc
```

**P2C:**

```bash
(echo '{"jsonrpc":"2.0","id":1,"method":"setPostPackConfirmationUuidBlocklist","params":[["PostPackConfig1"]]}'; sleep 1) \
  | socat - UNIX-CONNECT:admin.rpc

(echo '{"jsonrpc":"2.0","id":1,"method":"setPostPackConfirmationUuidBlocklist","params":[[]]}'; sleep 1) \
  | socat - UNIX-CONNECT:admin.rpc
```

> [!WARNING]
> **Clearing the blocklist**
>
> Use `params:[[]]`. Writing `params:[]` omits the parameter and the blocklist is **not** cleared.

---

## Related

- [tin_sample_servers](https://github.com/rakurai-io/tin_sample_servers) — sample Auth + Validator / Relayer servers
- [Tips](./tips.md) — tip accounts, virtual priority, TCA
- [Post-pack confirmations](./post_pack/README.md) — PSA / MCA
- [Using P2C](./post_pack/using_p2c.md) — packet layout, reply bundles
