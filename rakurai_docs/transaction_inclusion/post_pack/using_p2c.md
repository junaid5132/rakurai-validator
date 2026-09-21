# Using P2C

**This is the integration page for the post-pack stream: how to receive it, and how to act on it.** Once your [PSA](./psa.md) is funded, the validator connects out to your gRPC server and pushes each scheduled transaction to you at the point of no return. Your job is to decode it, decide whether there is an opportunity, and reply with a bundle before the block is public.

Three separate payments are in play and none substitutes for another: the **PSA** pays for the stream, the **MCA** shares the profit you make from it, and your reply bundle still needs a **tip** to be scheduled.

This page covers the Relayer gRPC services your server must expose, the packet layout and how to decode it, how to construct a valid reply bundle, and the validator Admin RPC for inspecting and blocklisting post-pack endpoints.

**Audience:** Searchers and TIN partners consuming the stream; validator operators managing endpoints.

---

## 1. Setup: Required gRPC services

To enable post-pack confirmations, run a gRPC **server** and share that URL with the Rakurai team on Slack or [Telegram](https://t.me/rakurai_official). Keep the matching **[PSA](./psa.md)** funded, or the stream stops.

Sample gRPC endpoint to share:

```
https://sample-server.com:20000
```

> [!NOTE]
> **The connection is outbound from the validator**
>
> The validator **connects** to your URL. You listen; the validator does not open a port for you. Bind the server so validators can reach it (public IP or resolvable hostname). `http://` and `https://` are both accepted.

**Required services** (protos: [`auth.proto`](../../../p2c-protos/protos/auth.proto), [`block_engine.proto`](../../../p2c-protos/protos/block_engine.proto)):

| Service | What the validator uses |
|---------|-------------------------|
| `auth.AuthService` | Role **`RELAYER`** (challenge signed by the validator identity, then a bearer token) |
| `block_engine.BlockEngineValidator` | `GetBlockEngineEndpoints` — P2C region autoconfig (`SubscribePackets` / `SubscribeBundles` may be `UNIMPLEMENTED` on a Relayer-only host) |
| `block_engine.BlockEngineRelayer` | Streams below |

| Relayer RPC | Required? | Notes |
|-------------|-----------|-------|
| `StartExpiringPacketStream` | **Yes** | Scheduler / post-pack txs. Without this, P2C never starts. |
| `StartExpiringTpuPacketStream` | Optional | Same `PacketBatchUpdate` msgs as the scheduler stream. Return `UNIMPLEMENTED` to skip. Source = **this RPC**, not a field on the packet. See also `meta.addr` differences in [§2.2](#22-meta-fields) and [Setup guide §5.1](../setup_guide.md#51-differentiating-scheduler-vs-tpu). |
| `StartP2cUpdateCountStream` | Optional | Per-slot `P2cUpdateCount` (uuid, slot, scheduler/tpu/total counts, `p2c_tpu_enabled`). Return `UNIMPLEMENTED` to skip. |

> [!NOTE]
> **Optional streams do not break the connection**
>
> If TPU or count RPCs are missing, newer validators keep `StartExpiringPacketStream` and continue. Sample flags: [`p2c_server`](https://github.com/rakurai-io/tin_sample_servers/tree/development/p2c_server) `--disable-tpu-packet-stream` / `--disable-p2c-update-count` (both streams **on** by default).

On-chain / Admin config can set `enable_tpu_p2c_update` per P2C URL. When false, the validator does not open the TPU stream even if your server implements it.

### 1.1. Reusing your block-engine URL

Partners often register **one URL** for both bundles (your server → validator) and P2C (validator → your server). That is supported, but the single server must then expose the Relayer role **as well as** the Validator role, or P2C silently never starts while bundles keep working.

The full service table and the failure mode are documented once, on the setup page: [Setup guide — same URL for bundles and P2C](../setup_guide.md#6-same-url-for-bundles-and-p2c). You may also register a **separate** P2C URL that implements Relayer + `GetBlockEngineEndpoints` (and leaves bundle subscribe RPCs unimplemented).

Once your endpoint is added, you receive transactions as `PacketBatch` (`solana_perf::packet::PacketBatch`) over the Jito packet gRPC protocol.

Rakurai will add partners' gRPC endpoints on-chain so you can receive updates from Rakurai nodes that have opted in. Using post-pack has **two money paths** — **[PSA](./psa.md) first**, **then [MCA](./mca.md)**.

---

## 2. Transaction / packet structure

[`packet.proto`](../../../p2c-protos/protos/packet.proto)

For each transaction, the validator sends one `PacketBatchUpdate` with `msg = batches` on either the **scheduler** or **TPU** Relayer stream (same shape; which stream you accepted is the source):

```
PacketBatchUpdate
  └── batches: ExpiringPacketBatch
        ├── header.ts
        ├── batch: PacketBatch
        │     └── packets[]: Packet
        │           ├── data    ← raw Solana wire transaction bytes
        │           └── meta: Meta
        │                 ├── size
        │                 ├── addr
        │                 ├── port
        │                 ├── flags: PacketFlags
        │                 └── sender_stake
        └── expiry_ms = <working-bank slot as u32>
```

> [!NOTE]
> **`expiry_ms` is the slot**
>
> On the P2C path, `expiry_ms` is **not** a millisecond expiry. It carries the validator **working-bank slot** as a `u32`.

### 2.1. Per-slot counts (`StartP2cUpdateCountStream`)

When enabled, the validator also pushes `P2cUpdateCount` once per slot (and flushes on disconnect):

| Field | Meaning |
|-------|---------|
| `uuid` | Post-pack endpoint UUID |
| `slot` | Slot these counts belong to |
| `scheduler_count` | Successful sends on `StartExpiringPacketStream` |
| `tpu_count` | Successful sends on `StartExpiringTpuPacketStream` |
| `total_count` | `scheduler_count + tpu_count` |
| `p2c_tpu_enabled` | Whether this endpoint has TPU updates enabled in config |


### 2.2. `meta` fields

`Meta` and `PacketFlags` are the packet gRPC types in [`packet.proto`](../../../p2c-protos/protos/packet.proto). Decode the transaction from `data`; treat `meta` as P2C fills it, not as a TPU/relayer packet.

| Field | Type | What the proto is | What P2C sends |
|-------|------|-------------------|----------------|
| `size` | `uint64` | Byte length of `data` | `data.len()` |
| `addr` | `string` | Source address on a normal packet | **Not an IP.** P2C copies a per-transaction string here. On the TPU path that string is the **validator identity signature** over the transaction signature (proof this leader emitted the update). Do not parse it as a socket address. |
| `port` | `uint32` | Source port | Always `0` |
| `flags` | `PacketFlags` | Per-packet flags (see below) | **Omitted** (`None`) — treat as unset / all false |
| `sender_stake` | `uint64` | Stake of the sending node | Always `0` |

`PacketFlags` (present in the proto, not set on P2C):

| Flag | Meaning on a normal packet |
|------|----------------------------|
| `discard` | Packet should be dropped |
| `forwarded` | Already forwarded |
| `repair` | Repair traffic |
| `simple_vote_tx` | Simple vote |
| `tracer_packet` | Tracer |
| `from_staked_node` | Came from a staked node |

Do not use `sender_stake`, `port`, or `flags` to decide whether to backrun. When you reply, put the original `Packet` **unchanged** — `data` and `meta` — as the first packet(s) of the bundle.

**Decode in Rust:**

```rust
use solana_transaction::versioned::VersionedTransaction;

let txn: VersionedTransaction = bincode::deserialize(&packet.data)?;
```

`packet.data` is the raw Solana wire transaction. Decode it, inspect accounts and instructions, then decide whether to backrun.

---

## 3. Send a bundle

Receive the post-pack confirmation `Packet`, then send back a **bundle** (`SendBundle` through block-engine).

When building the bundle, include:

1. The original post-pack confirmation packet(s) **unchanged** (the same `Packet` you received)
2. Any additional transactions (e.g., backrun / arbitrage)
3. A transaction or instruction with a **tip** to one of [Rakurai’s tip accounts](../tips.md#appendix-program-and-tip-account-addresses)

> [!TIP]
> **Recommended tip for high prioirty transactions (Backrun/Mev)**
>
> **0.001 SOL** as tip in your backrun bundle, — see [Tips](../tips.md). Bundles that use post-pack confirmations receive an additional priority boost.

A `Bundle` is a header plus a list of `Packet`s — the same packet shape as the stream:

```
Bundle
  ├── header
  └── packets[]: Packet     ← original post-pack packet(s) first, then your txs
```

---

## 4. Commands

**Searcher / TIN partner (on-chain money paths):**

| What | CLI |
|------|-----|
| Fund and inspect **PSA** | [`rakurai-p2c`](../../rakurai_programs/cli/p2c_subscription.md) |
| Report and settle **MCA** | [`rakurai-revshare`](../../rakurai_programs/cli/partner_reward_settlement.md#22-post-pack-mevshare----revenue-kind-mev-share) |

**Validator operators** use Admin RPC below to inspect which post-pack endpoints are active and to blocklist services. **Registering** a new endpoint (adding your gRPC URL on-chain) is not done here — share your endpoint with the Rakurai team on Slack or [Telegram](https://t.me/rakurai_official); see [Setup guide](../setup_guide.md#1-contact-rakurai-and-share-your-wallet-pubkey).

Admin IPC is request/response: keep the socket open briefly so `socat` can read the reply before stdin closes.

### 4.1. getPostPackConfirmationConfig

Returns the live status maintained by the scheduler (admin + on-chain merge, blocklist, and what is actually connected).

| Field | Description |
|-------|-------------|
| `onchain_entries` | Entries loaded from the on-chain PDA (`url`, `uuid`, `enable_tpu_p2c_update`) |
| `blocklisted_uuids` | Endpoint UUIDs blocked via `setPostPackConfirmationUuidBlocklist` |
| `blocklisted_entries` | Full merged entries whose `uuid` is blocklisted |
| `active_entries` | Merged admin + on-chain (admin wins on same URL), excluding blocklisted UUIDs — these are the endpoints receiving scheduler updates (and TPU when `enable_tpu_p2c_update` is true) |

```bash
(echo '{"jsonrpc":"2.0","id":1,"method":"getPostPackConfirmationConfig","params":[]}'; sleep 1) \
  | socat - UNIX-CONNECT:admin.rpc | jq
```

**Example response:**

```json
{
  "admin_entries": [
    {"url":"http://127.0.0.1:20000","uuid":"PostPackConfig2","enable_tpu_p2c_update":true},
    {"url":"http://127.0.0.1:10000","uuid":"PostPackConfig1","enable_tpu_p2c_update":false}
  ],
  "onchain_entries": [],
  "blocklisted_uuids": ["PostPackConfig1"],
  "blocklisted_entries": [
    {"url":"http://127.0.0.1:10000","uuid":"PostPackConfig1","enable_tpu_p2c_update":false}
  ],
  "active_entries": [
    {"url":"http://127.0.0.1:20000","uuid":"PostPackConfig2","enable_tpu_p2c_update":true}
  ]
}
```

### 4.2. setPostPackConfirmationUuidBlocklist

Blocklists post-pack confirmation endpoints by **UUID**. Each call **replaces** the full blocklist. Pass an empty array to clear.

Blocklisted UUIDs are removed from `active_entries` on the next scheduler config sync. If a blocklisted endpoint already has an open gRPC connection, it is torn down immediately on sync; other endpoints stay connected.

**Example — block one endpoint by UUID:**

```bash
(echo '{"jsonrpc":"2.0","id":1,"method":"setPostPackConfirmationUuidBlocklist","params":[["PostPackConfig1"]]}'; sleep 1) \
  | socat - UNIX-CONNECT:admin.rpc
```

**Example — clear blocklist (reconnect blocklisted endpoints on next sync):**

```bash
(echo '{"jsonrpc":"2.0","id":1,"method":"setPostPackConfirmationUuidBlocklist","params":[[]]}'; sleep 1) \
  | socat - UNIX-CONNECT:admin.rpc
```

> [!WARNING]
> **Clearing the blocklist**
>
> `setPostPackConfirmationUuidBlocklist` takes **one** parameter: an array of UUIDs. To clear the blocklist you must pass an empty array *inside* the parameter list — `params:[[]]`. Writing `params:[]` omits the parameter entirely and the blocklist is **not** cleared.

---

## Related

- [Tips](../tips.md) — tip for reply bundles and virtual priority
- [PSA](./psa.md) · [MCA](./mca.md)
- [Setup guide](../setup_guide.md) — Validator gRPC and discovery
