# PSA — P2C Subscription Account

**The P2C Subscription Account (PSA) is what you prepay to keep a validator's post-pack stream enabled.**.

The fee is derived from that validator's **staked SOL**, which is public, so you can work out what each validator costs before you subscribe. You fund it up front, the epoch's fee is deducted when that epoch closes, and if you stop consuming you close the account and recover the unused balance. Underfunding gives you a grace window rather than an instant cut-off.

A PSA is not a tip and not a share of backrun profit. It is an **access fee**, paid in advance, for one service against one validator. How it compares with the MCA and with tips: [the three payments, side by side](./README.md#2-the-three-payments-side-by-side).

**Audience:** Searchers, traders, and TIN partners who consume post-pack confirmations.

---

## 1. Why the PSA exists and how it is priced

Post-pack is a live gRPC stream of transactions at the point of no return. Delivering it costs the validator, so it is priced rather than free.

- **One PSA per service, per validator.** You pay for each validator whose updates you receive. Running against ten validators means ten PSAs under the same service name.
- **The fee is derived from that validator's staked SOL.** At the end of each epoch the program writes a **stake snapshot** into the epoch entry and prices the fee from it. Stake is public, so you can estimate the cost per validator before you subscribe.
- **The fee splits two ways:** a commission to Rakurai (`commission_bps`), and the remainder to the validator.

If `block_reward_conversion_enabled` is on (the default), that validator remainder is later converted into a high-priority block reward. If you index Rakurai revenue, do not count it twice — see the [indexing note](../README.md#6-do-not-double-count-tips-and-converted-block-rewards).

---

## 2. Account status and the grace window

Every PSA carries a status that determines whether the stream keeps flowing.

| Status | Meaning | Stream |
|--------|---------|--------|
| `Active` | Fees are current | Delivered |
| `InGrace` | One or more recent epochs were underfunded, still inside the grace window | Delivered |
| `Suspended` | The unpaid streak exceeded `grace_epochs` | **Stopped** |

The program counts consecutive underfunded epochs in `unpaid_streak` and compares it against `grace_epochs`, which defaults to **2**. Any shortfall is also accumulated in `deficit`.

> [!CAUTION]
> **Suspension stops delivery until the deficit is cleared**
>
> Once the status reaches `Suspended`, the validator stops sending post-pack updates to your endpoint. Funding the shortfall clears the deficit and restores delivery, but the updates you missed while suspended are gone — there is no replay.

> [!WARNING]
> **A healthy-looking balance can still be short**
>
> `total owed` and the `fund-all` shortfall both cover **past epochs only**. The in-progress epoch has not been priced yet, so keep a buffer above the displayed figure rather than funding to exactly zero.

---

## 3. Lifecycle

1. **Onboard.** Contact the Rakurai team on Slack or [Telegram](https://t.me/rakurai_official) and share your gRPC endpoint — see [Setup guide](../setup_guide.md#1-contact-rakurai-and-share-your-wallet-pubkey) and [Using P2C](./using_p2c.md#1-setup-required-grpc-services).
2. **Rakurai opens the account.** A PSA is created for your service name against each validator, and your endpoint is registered. Defaults (manager, commission, grace) are copied from the on-chain `P2CConfigAccount`.
3. **You fund it.** Top up with [`rakurai-p2c fund`](../../rakurai_programs/cli/p2c_subscription.md#33-fund), `fund-all` across every validator at once, or a plain `solana transfer` to the PDA — anyone may fund it.
4. **The epoch closes.** The stake snapshot is written and the fee is deducted from the prepaid balance: Rakurai's commission first, remainder to the validator.
5. **You keep it topped up.** Check status and deficit each epoch and fund before the streak reaches the grace limit.
6. **You leave.** Once every billed epoch is paid, the account can be closed and the leftover prepaid balance is returned.

---

## 4. Checking your account

```sh
rakurai-p2c \
  --url <RPC_URL> \
  --program-id <REWARD_DISTRIBUTION_PROGRAM_ID> \
  get-account \
  --name <SERVICE_NAME> \
  --vote-pubkey <VALIDATOR_VOTE_PUBKEY>
```

This prints the derived PDA, balance after rent exemption, total owed for past epochs, status, and the deficit when it is above zero. Add `--detail` for per-epoch rows (`due` / `deducted` / `owed` / `claimed`) and the commission settings.

Use `get-all-accounts --name <SERVICE_NAME>` to see every validator you subscribe to in one table.

Full command reference: **[P2C Subscription CLI (`rakurai-p2c`)](../../rakurai_programs/cli/p2c_subscription.md)**. On-chain struct: [Reward Distribution — PSA](../../rakurai_programs/programs/reward_distribution/README.md#72-psa--p2csubscriptionaccount).

---

## 5. Related

- [MCA](./mca.md) — share backrun profit after you have stream access
- [Using P2C](./using_p2c.md) — Relayer gRPC stream and reply bundles
- [Tips](../tips.md) — landing tips are a separate path from the PSA
