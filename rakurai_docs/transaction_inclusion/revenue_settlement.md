# Revenue settlement

**This is the quick checklist for settling what you owe on TIN** — tips (TCA), prepaid post-pack (PSA), and backrun / MevShare (MCA).

For **detailed CLI flags and examples**, see the CLI docs linked in each section (and in the table below). Download the latest CLI from the [releases](https://github.com/rakurai-io/rakurai_programs/releases/latest).

Revenue flow: [TIN revenue streams](./README.md#2-revenue-streams). Tip mechanics: [Tips](./tips.md).

**Audience:** Traders and landing services tipping for inclusion, and TIN partners on post-pack (PSA + MCA).

| Stream | Account | CLI | What you do |
|--------|---------|-----|-------------|
| Tips (Rakurai tip accounts) | **TCA** | — | Nothing — settlement is automatic |
| Tips (partner tip account) | **TCA** | [`rakurai-revshare`](../rakurai_programs/cli/partner_reward_settlement.md) `--revenue-kind Tip` | Transfer after the epoch (interim; prefer Rakurai tip accounts) |
| Post-pack access | **PSA** | [`rakurai-p2c`](../rakurai_programs/cli/p2c_subscription.md) | Keep prepaid funded |
| Backrun / MEV | **MCA** | [`rakurai-revshare`](../rakurai_programs/cli/partner_reward_settlement.md) `--revenue-kind Mev-share` | Record, then transfer |

> [!CAUTION]
> **Settle within 2 epochs**
>
> Partner tip TCA balances, P2C subscriptions, and backrun revenue must be settled within 2 epochs. Otherwise, the related TIN service may stop. Rakurai tip accounts do not need this manual step.

---

## 1. TCA — Tips Collection Account

Tips for scheduling priority settle into a per-service, per-validator **TCA**. Prefer the **Rakurai tip accounts** path — settlement is automatic. A partner tip account is interim only and requires manual settlement each epoch.

### 1.1. Rakurai Tip Account (default & recommended)

Traders and transaction landing services tip any of [Rakurai’s eight tip accounts](./tips.md#appendix-program-and-tip-account-addresses). **Nothing to settle from the trader or landing-service side.** Tip into a Rakurai tip account and you’re done. (**Minimum recommended tip: `0.001 SOL`**)

### 1.2. Partner Tip Account (interim — prefer Rakurai tip accounts)

Registering your **own tip account** is interim support only. The flow is more complex than Rakurai tip accounts (manually transfer each epoch). **Use [Rakurai tip accounts](./tips.md#appendix-program-and-tip-account-addresses)**

If you still use a partner tip account: traders tip into that account; tips stay with you — Rakurai cannot drain them.

- **60% of the total tip** is used for virtual priority
- Each leader turn the validator **records** that 60% in the TCA (no SOL moves yet)
- After the epoch, **you transfer** the owed SOL into the TCA

#### 1.2.1. Each epoch

- Wait for the epoch to end
- Use the [`rakurai-revshare` CLI](https://github.com/rakurai-io/rakurai_programs/releases/latest)
- Inspect: `get-all-accounts` (`--revenue-kind Tip`) — view any pending record
- Settle: `transfer-all` (`--dry-run` first if needed) — settle the owed revenue

Detailed commands: [Tip and MevShare Settlement CLI](../rakurai_programs/cli/partner_reward_settlement.md). Registration: [custom tip accounts](./tips.md#4-can-i-use-my-own-tip-account-instead-of-rakurais-eight-accounts).

---

## 2. PSA — P2C Subscription

The **PSA** is the prepaid subscription fee that keeps a validator’s post-pack (P2C) stream enabled. One PSA per **service**, per **validator**. The fee is priced from that validator’s **staked SOL** and deducted when the epoch ends.

- If you are **selling P2C to searchers**, share **60-70%** of that subscription revenue through TIN
- Expected baseline: **$500 / month / 1M SOL stake** (dollar equivalent amount in SOL)
- Use your own **RSMS / Revenue Recognition / Attribution System** so you can provide audit report on Rakurai / validator request

### 2.1. Each epoch

- Use the [`rakurai-p2c` CLI](https://github.com/rakurai-io/rakurai_programs/releases/latest)
- Fund **before** the epoch ends: `fund-all`, or `solana transfer` to the PDA — keep the prepaid balance funded
- Wait for the epoch to end
- Inspect: `get-all-accounts` — view balance, total owed, status (`Active` / `InGrace` / `Suspended`), and any deficit
- Settle / top up: `fund-all` (`--dry-run` first if needed) — clear shortfalls so the stream stays active

> [!NOTE]
> **Suspended stops delivery**
>
> Status `Suspended` means the stream has stopped until the shortfall is cleared. Updates missed while suspended are not replayed.

Detailed commands: [P2C Subscription CLI](../rakurai_programs/cli/p2c_subscription.md).

---

## 3. MCA — MevShare

After the PSA is funded, you consume P2C and can backrun. Profit stays with you during the epoch; you share it through the **MCA**. Settlement is **self-reported** — you must hold the MCA `record_authority`.

- Backrun bundles using **P2C source transactions** get a **20% virtual priority boost**
- Share **60-70%** of backrun / MEV revenue through TIN
- Expected baseline: **60-70 SOL / month / 1M stake**
- Use your own **RSMS / Revenue Recognition / Attribution System** so you can provide audit report on Rakurai / validator request

### 3.1. Each epoch

- Wait for the epoch to end
- Use the [`rakurai-revshare` CLI](https://github.com/rakurai-io/rakurai_programs/releases/latest)
- Record: `record-revenue` **once** per validator (`--revenue-kind Mev-share`, `record_authority` keypair) — ledger only, no SOL moves
- Inspect: `get-all-accounts` (`--revenue-kind Mev-share`) — view any pending record
- Settle: `transfer-all` (`--dry-run` first if needed) — settle the owed revenue

> [!WARNING]
> **Record once per validator per epoch**
>
> `record-revenue` **adds** to the epoch entry. Running it twice books twice what you owe — check with `get-all-accounts` before recording again.

Detailed commands: [Tip and MevShare Settlement CLI](../rakurai_programs/cli/partner_reward_settlement.md).