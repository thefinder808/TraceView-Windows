# Releasing TraceView

Pushing a tag like `v0.9.0` runs [`release.yml`](../.github/workflows/release.yml):

1. **Build**, with no access to signing: test everything, then build `traceview.exe` for x64 and ARM64 with [`scripts/build-release.ps1`](../scripts/build-release.ps1). The tag has to match the version in `Cargo.toml`.
2. **Release**, in the `release` environment:
   - sign both EXEs with Azure Artifact Signing, the account WinSign uses;
   - check each signature and its timestamp;
   - zip each EXE with `LICENSE` and `THIRD-PARTY-NOTICES.txt`, and write `SHA256SUMS.txt`;
   - create a **draft** GitHub release with those files.

Nothing is published until the draft is checked and published by hand. A release is never made unsigned: if signing isn't set up, the run stops.

## Cutting a release

1. Set `version` in the root `Cargo.toml`, run `cargo build` so `Cargo.lock` follows, and add the release to `CHANGELOG.md`. Merge that to `main`.
2. Tag `main` and push the tag:

   ```bash
   git tag v0.9.0
   ```

   ```bash
   git push origin v0.9.0
   ```

3. When the run finishes, open the draft release and download a zip. Check that Explorer shows a valid signature under Properties > Digital Signatures, and try the EXE.
4. Publish the draft. Releases before 1.0 are marked as pre-releases.

`pwsh -File scripts/build-release.ps1` makes the same zips locally, unsigned, for trying a build. It warns that they're unsigned.

## One-time setup

The workflow signs in to Azure with OpenID Connect. GitHub proves to Azure that the request comes from this repository's `release` environment, so no Azure secret is stored anywhere.

### In Azure

These use the app registration and Artifact Signing account WinSign already signs with.

1. **Trust this repository.** In Microsoft Entra ID > App registrations, open the app WinSign's release signs with. Here it's named `github-actions-codesign`. The signing account's Access control (IAM) > Role assignments shows which app holds the signer role, if in doubt. Then go to Manage > Certificates & secrets > Federated credentials > Add credential:
   - Scenario: GitHub Actions deploying Azure resources
   - Organization: `thefinder808`, with Organization ID `46229529`. For a personal account it's the account's ID, also in its noreply email address.
   - Repository: `TraceView-Windows`, with Repository ID `1440198878`
   - Entity type: Environment, named `release`
   - Name: `traceview-windows-release`

   The subject identifier should then read `repo:thefinder808@46229529/TraceView-Windows@1440198878:environment:release`. The IDs never change, so renaming the account or repository, or someone later taking an old name, can't reuse the trust. It matches only this repository's `release` environment. GitHub's tokens for this repository already use that form, which `gh api repos/thefinder808/TraceView-Windows/actions/oidc/customization/sub` shows as `use_immutable_subject: true`. Both IDs come from `gh api repos/thefinder808/TraceView-Windows`.
2. **Signing rights.** The app's service principal needs the Artifact Signing Certificate Profile Signer role on the signing account. WinSign's already has it, if that's the account in use.

### In GitHub

1. **Create the `release` environment** (Settings > Environments > New environment) and, under Deployment branches and tags, allow only the tag pattern `v*`. That way only a release tag can reach the signing settings. If the page offers Required reviewers, turn it on for yourself, so each release waits for your approval.
2. **Add these environment secrets** to `release`. None is a password, but keeping them as secrets keeps them out of logs.

   | Secret | What |
   |---|---|
   | `AZURE_CLIENT_ID` | The app registration's Application (client) ID |
   | `AZURE_TENANT_ID` | The Directory (tenant) ID |
   | `TRUSTED_SIGNING_ENDPOINT` | The signing account's endpoint, like `https://wus2.codesigning.azure.net` |
   | `TRUSTED_SIGNING_ACCOUNT` | The signing account's name |
   | `TRUSTED_SIGNING_PROFILE` | The certificate profile's name |

   For example, with `gh`, which asks for each value:

   ```bash
   gh secret set AZURE_CLIENT_ID --env release --repo thefinder808/TraceView-Windows
   ```

## Hardening

- **Signing is kept apart from the build.** The build job has no secrets and no Azure sign-in, so code that runs while building, such as a crate's build script, can't reach them. It uses no cache, so a release is built only from the tag and crates.io.
- **Actions are pinned to exact commits** in every workflow, so a moved tag can't change what runs.
- **Only tag pushes trigger it.** Pull requests never get the environment's secrets, and nothing that reacts to pull requests should be added to it.
- **Least privilege.** The workflow's token can only write releases. The Azure app can only sign, with the one certificate profile.
