# Rakurai Binary Attestation — Guide

**The Rakurai scheduler ships as a prebuilt binary, so you are running code you did not compile.** Attestation is how you check that what you downloaded is the artifact Rakurai's CI produced from Rakurai's source.

The binary carries a [GitHub artifact attestation](https://docs.github.com/en/actions/concepts/security/artifact-attestations): a signed claim of build provenance, verified with the GitHub CLI in a single command, online or on an air-gapped host. It records which repository, workflow, and commit produced the file. Treat a failed verification as a hard stop.

**Audience:** Validator operators who want to cryptographically verify scheduler binaries before deployment.

---

## 1. GitHub artifact attestations

The Rakurai scheduler binary comes with [GitHub artifact attestation](https://docs.github.com/en/actions/concepts/security/artifact-attestations), providing cryptographic proof of its build provenance and integrity.

Artifact attestations enable unfalsifiable provenance and integrity guarantees for software. Software consumers can verify where and how the software was built. GitHub's artifact attestations create cryptographically signed claims that establish build provenance and include:

- A link to the workflow associated with the artifact
- The repository, organization, environment, commit SHA, and triggering event for the artifact

For more information, see [Artifact attestations](https://docs.github.com/en/actions/concepts/security/artifact-attestations).

---

## 2. SLSA compliance

GitHub artifact attestations are SLSA compliant. The SLSA framework is an industry standard used to evaluate supply chain security. This gives you confidence that the binary has not been tampered with after the build and can be securely traced back to its source.

For more information, see [SLSA](https://slsa.dev/).

---

## 3. Verify with GitHub CLI

Rakurai generates an attestation for every release. The following steps show how to verify it using the GitHub CLI.

### 3.1. Prerequisites

- [GitHub CLI](https://github.com/cli/cli#installation) installed

### 3.2. Verification

To verify the Rakurai scheduler binary, use the following GitHub CLI command.

> [!NOTE]
> **Air-gapped hosts need a different flow**
>
> This command reaches out to the GitHub API. On an offline or air-gapped machine, follow [Verifying attestations offline](https://docs.github.com/en/actions/how-tos/secure-your-work/use-artifact-attestations/verify-attestations-offline) to bundle the attestation alongside the binary first.

```bash
gh attestation verify PATH/TO/RAKURAI/SCHEDULER/BINARY -R rakurai-io/rakurai-validator
```

### 3.3. Results

Check the command output to determine the result. If verification is successful, you will see a message like `✓ Verification succeeded!`. If verification fails, you will see an error message such as `✗ Loading attestations from GitHub API failed`.

#### 3.3.1. Successful verification

You will see the enforced policy, a checkmark, and one or more matched attestations summarizing the build and signer. Example:

```bash
$ gh attestation verify ./PATH/TO/BINARY -R rakurai-io/rakurai-validator
Loaded digest sha256:<ARTIFACT_DIGEST> for file://...
Loaded <N> attestations from GitHub API

Policy criteria enforced:
- Predicate type: https://slsa.dev/provenance/v1
- Source repository owner: https://github.com/rakurai-io
- Source repository: https://github.com/rakurai-io/rakurai-validator
- Subject Alternative Name: ^https://github.com/rakurai-io/rakurai-validator/
- OIDC issuer: https://token.actions.githubusercontent.com

✓ Verification succeeded!

Matched attestations:
- Attestation #<NUMBER>
- Build repo: rakurai-io/rakurai-validator
- Build workflow: .github/workflows/<WORKFLOW_FILE>@refs/tags/<TAG>
- Signer repo: rakurai-io/rakurai-validator
- Signer workflow: .github/workflows/<WORKFLOW_FILE>@refs/tags/<TAG>
```

#### 3.3.2. Failed verification

A failure result means no attestation could be found (HTTP 404), and the binary's authenticity could not be confirmed. Example:

> [!CAUTION]
> **Do not deploy a binary that fails verification**
>
> A failed check means the artifact cannot be traced back to a Rakurai build. Treat it as untrusted: delete it, re-download from the [official endpoint](./setup_and_build.md#233-download-the-scheduler-binary), and verify again. If it still fails, contact the Rakurai team on [Telegram](https://t.me/rakurai_official) before running it on a validator.

```bash
$ gh attestation verify ./PATH/TO/BINARY -R rakurai-io/rakurai-validator
Loaded digest sha256:<artifact-digest> for file://...
✗ Loading attestations from GitHub API failed

Error: HTTP 404: Not Found
```
