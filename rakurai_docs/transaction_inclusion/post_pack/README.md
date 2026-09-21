# Post-Pack Confirmations

**Rakurai Pre-confirmation named as Post-pack confirmations (P2C) provides you a front-run-proof early view of what is about to execute.** It is the third [TIN building block](../README.md#13-post-pack-confirmations-p2c): the Rakurai scheduler streams each transaction to you at the **point of no return** — committed to the block being built, but not yet public.

Paying for it has two ordered halves. You **prepay for access** through a [PSA](./psa.md), priced from the validator's stake, then **share a percentage of the profit** you make from the stream through an [MCA](./mca.md). Tips are a separate path and still apply to your reply bundles.

This page covers what the stream is, the wire protocol it uses, the end-to-end sequence from onboarding to settlement, and how the three money paths differ.

**Audience:** Searchers, TIN partners, traders consuming post-pack confirmations, and validator operators configuring endpoints.

---

## 1. What are post-pack confirmations?

As soon as a transaction has been scheduled for execution, the Rakurai scheduler forwards it over gRPC to every configured **post-pack confirmation endpoint** — one packet per transaction, per endpoint.

> [!NOTE]
> **Updates start at the point of no return**
>
> Updates are generated from the **point of no return** — the moment the transaction is committed to the block being built. Consumers therefore see a transaction only when it is too late for anyone to front-run it, which is what makes it safe to publish. A post-pack signal is an *early scheduled* update, not a confirmation: confirm landing through standard commitment checks before treating it as final.

### 1.1. Protocol

Post-pack uses the TIN / P2C gRPC packet protocol ([`packet.proto`](../../../p2c-protos/protos/packet.proto), [`block_engine.proto`](../../../p2c-protos/protos/block_engine.proto)) — wire-compatible with the Jito `Packet` / `PacketBatch` shapes. If you already consume a Jito relayer stream, the layout is familiar.

- **Scheduler stream:** `StartExpiringPacketStream` — point-of-no-return txs (required).
- **TPU stream:** `StartExpiringTpuPacketStream` — same message types; optional (`UNIMPLEMENTED` is fine). Tell scheduler vs TPU apart by **which RPC** you opened, not by a field on the packet.
- **Count stream:** `StartP2cUpdateCountStream` — per-slot send counts; optional.
- **Discovery:** `GetBlockEngineEndpoints` on the P2C host so validators can autoconfig / pick a region (same RPC as bundles).
- **`expiry_ms`:** working-bank **slot** as `u32` (not a millisecond expiry).
- **What you send back:** a **bundle** containing the original post-pack packet(s) unchanged, followed by any transactions of your own (typically a backrun or arbitrage).

Full mechanics, packet layout, and Admin RPC: [Using P2C](./using_p2c.md). Onboarding / Relayer setup: [Setup guide — BlockEngineRelayer](../setup_guide.md#5-set-up-blockenginerelayer-p2c).

### 1.2. End-to-end sequence

```
you register a gRPC endpoint with Rakurai       → Setup guide
        ↓
Rakurai creates your PSA (and MCA if sharing)   → PSA / MCA
        ↓
you fund the PSA                                → rakurai-p2c fund
        ↓
validator connects out to your Relayer server   → Using P2C
        ↓
scheduler streams PacketBatchUpdate at the
point of no return
        ↓
you decode, decide, and reply with a bundle
(original packets + your txs + a tip)           → Tips
        ↓
epoch ends → PSA fee deducted from prepaid
          → you report and settle MCA           → rakurai-revshare
```

> [!TIP]
> **Still tip on reply bundles**
>
> Paying a tip to land a transaction is a **different path** from the PSA and MCA. A post-pack reply bundle still needs a tip to a [Rakurai tip account](../tips.md#appendix-program-and-tip-account-addresses) so it gets prioritized — **recommended: 1,000,000 lamports (0.001 SOL)**. See [Tips](../tips.md).

---

## 2. The three payments, side by side

Post-pack touches three money paths and they are easy to confuse. **PSA and MCA are ordered** — access first, profit share second. Tips are independent of both.

| | **PSA** | **MCA** | **Tips (TCA)** |
|--|---------|---------|----------------|
| Full name | P2C Subscription Account | MevShare Collection Account | Tips Collection Account |
| What it buys | **Access** to the stream | Nothing — it is a **payout** you owe | **Priority** for one transaction or bundle |
| Direction | You prepay; the fee is deducted each epoch | You send your agreed share after the epoch | You transfer at send time |
| Priced by | The validator's **staked SOL** | Your agreed **percentage of backrun profit** | Whatever you choose to tip |
| Required? | **Yes**, for post-pack at all | Yes, once you backrun and share | Optional, but recommended |
| Tool | [`rakurai-p2c`](../../rakurai_programs/cli/p2c_subscription.md) | [`rakurai-revshare`](../../rakurai_programs/cli/partner_reward_settlement.md) | Plain `SystemProgram.transfer` |
| Guide | [PSA](./psa.md) | [MCA](./mca.md) | [Tips](../tips.md) |

> [!NOTE]
> **PSA first, then MCA**
>
> The MCA only applies **after** you already have stream access, because it shares profit you made *from* that stream. You cannot skip the PSA and go straight to the MCA.

Endpoints — where the scheduler **sends** you transactions — are stored separately in [Client Config](../../rakurai_programs/programs/rakurai_client_config/README.md). The PSA holds prepaid SOL; the MCA holds settled backrun SOL. Neither holds endpoint configuration.

> [!WARNING]
> **Do not count the same SOL twice**
>
> If `block_reward_conversion_enabled` is on (the default), PSA, MCA, and TCA claims are later re-emitted as a high-priority block reward. An indexer watching both will count the same lamports twice. See [TIN — indexing note](../README.md#6-do-not-double-count-tips-and-converted-block-rewards).

---

## 3. Next steps

| Guide | Description |
|-------|-------------|
| [Using P2C](./using_p2c.md) | Relayer gRPC setup, packet structure, reply bundles, validator Admin RPC |
| [PSA](./psa.md) | Prepaid stream access: pricing, status, grace window, funding |
| [MCA](./mca.md) | Reporting and settling your backrun share |
| [Tips](../tips.md) | Tip accounts and virtual priority for landing |
| [Setup guide](../setup_guide.md) | Discovery endpoint and Validator vs Relayer gRPC roles |
| [Reward Distribution](../../rakurai_programs/programs/reward_distribution/README.md) | The on-chain model behind PSA, MCA, and TCA |
