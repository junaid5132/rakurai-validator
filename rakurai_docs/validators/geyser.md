# Rakurai Geyser — Guide

**If you run a Geyser plugin, read this before upgrading to Rakurai.** A plugin built against stock Agave will not work on a Rakurai node, and it may crash the validator rather than fail cleanly.

The fix is a `[patch.crates-io]` entry pointing your Geyser interface crates at the Rakurai repository, plus a clean rebuild — a dependency change, not a code change. It has to be redone on every Rakurai release, because the patch tag must match the validator version.

**Audience:** Validator operators running third-party Geyser plugins with Rakurai.

---

## 1. Overview

The struct layout and padding of the **Rakurai Validator** are slightly different from the standard **Agave/Solana validator**. Because of this, you cannot directly run any Geyser with the Rakurai validator and vice versa. To run any Geyser with Rakurai Validator, you must use the Geyser-related crates from the Rakurai repository and build your Geyser with them.

A sample Geyser plugin binary is provided in this repository: **[spark-geyser](../../spark-geyser/README.md)**.

> [!CAUTION]
> **Never run an unpatched Geyser**
>
> Running a standard Geyser without applying the Rakurai-compatible patches **may crash your node or cause undefined behavior**. Always build against the Geyser crates from the Rakurai repository.

---

## 2. Required patch

Apply the following patch to your Geyser's top-level `Cargo.toml`:

```toml
[patch.crates-io]
agave-geyser-plugin-interface = { git = "https://github.com/rakurai-io/rakurai-validator.git", package = "agave-geyser-plugin-interface", tag = "release/v3.1.8-rakurai.0" }
solana-account-decoder = { git = "https://github.com/rakurai-io/rakurai-validator.git", package = "solana-account-decoder", tag = "release/v3.1.8-rakurai.0" }
solana-transaction-context = { git = "https://github.com/rakurai-io/rakurai-validator.git", package = "solana-transaction-context", tag = "release/v3.1.8-rakurai.0" }
solana-transaction-status = { git = "https://github.com/rakurai-io/rakurai-validator.git", package = "solana-transaction-status", tag = "release/v3.1.8-rakurai.0" }

[workspace.lints.rust]
deprecated = "allow"
```

> [!CAUTION]
> **The release tag must match your validator version**
>
> The `tag` in every patch entry must exactly match the Rakurai validator version you run. A mismatch causes a struct/ABI mismatch that **may crash the validator**.

---

## 3. Build steps

After applying the patch:

```bash
cargo clean
cargo build --release
```

`cargo clean` is required, not optional: the patch changes which source the Geyser interface crates resolve to, and a stale `target/` directory can silently link the previous Agave-built objects.

Then run your Geyser plugin as usual.

---

## 4. Verify the build

Confirm the patch actually took effect before loading the plugin on a validator:

```bash
# Every agave-geyser-plugin-interface entry should resolve to the Rakurai git source,
# not to a registry (crates.io) source.
cargo tree -i agave-geyser-plugin-interface
```

If any entry still shows `registry+https://github.com/rust-lang/crates.io-index`, the patch did not apply — check that `[patch.crates-io]` is in the **top-level** `Cargo.toml` of your workspace rather than in a member crate.

---

## 5. Upgrading

Each time you upgrade the Rakurai validator:

1. Update every `tag` in the `[patch.crates-io]` block to the new release.
2. `cargo clean && cargo build --release`.
3. Replace the plugin `.so` and restart the validator with the plugin loaded.

Rebuild the Geyser plugin **before** restarting the validator on the new release, so the two never run against mismatched struct layouts.

---

## 6. Reference implementation

For a working plugin and its configuration, see [spark-geyser](../../spark-geyser/README.md) — a prebuilt, lightweight ZeroMQ forwarding plugin maintained alongside each Rakurai release.
