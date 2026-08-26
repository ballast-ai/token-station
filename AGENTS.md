# Token Station Engineering Rules

## Use English by default

Use English for new and modified technical content. This rule applies to code comments, documentation,
design records, test names, commit messages, pull requests, release notes, and contributor instructions.

Use ASCII English words for technical-document filenames. Locale suffixes such as `zh-CN` are allowed,
but localized words are not.

Keep commands, paths, protocol fields, API names, and code identifiers unchanged. Keep required localized
user-interface text in its target language. You can quote an error or log entry in its original language.

If a document must contain English and Simplified Chinese, put the complete English text first. Put the
complete Simplified Chinese text after it.

Write commit messages in English. Use a short imperative subject. Describe one logical change in each
commit.

## Commit directly to main

Work on the `main` branch. Commit authorized changes directly to `main`.

Do not create feature, topic, release, or migration branches unless the user explicitly requests one.
Do not create a pull request unless the user explicitly requests one.

After you complete a feature or task, commit all authorized changes locally. Use an English commit
message.

Before each commit, verify that the current branch is `main`. Stage only the files authorized for that
commit. Keep unrelated working-tree changes unstaged.

## Use Simplified Technical English

Use strict Simplified Technical English for procedures, runbooks, safety warnings, and error messages.

- Put one instruction in each sentence.
- Put the condition before the action.
- Use the imperative form for instructions.
- Keep an instruction at 20 words or fewer when practical.
- Keep a descriptive sentence at 25 words or fewer when practical.
- Use active voice when the actor is known.
- Do not use contractions or semicolons.
- Use one stable name for one thing.
- Prefer short and common words.

Use STE-flavored English for READMEs, design documents, pull requests, release notes, and general technical
explanations. Keep the text direct and natural. Remove filler, marketing claims, and unnecessary abstract
terms.

Do not apply STE rules to code, identifiers, command syntax, or required localized text.

## Write the design document first

Create or update a design record in the private `ballast-ai/token-station-doc` repository before you change any of these items:

- User-visible interfaces.
- Interaction flows.
- State models.
- Frontend and backend contracts.
- Release behavior.

Review the design record in the private repository before implementation. Keep all non-public design,
planning, review, incident, and operational documents in that private repository.

Do not create `docs/design/` in this public repository. Do not copy internal documents into this public
repository, its commits, its branches, or its pull requests. Keep only user-facing documentation and
source-required public records in this repository.

Include these sections in the design document:

1. Problem, goal, scope, and non-goals.
2. Security and data boundaries.
3. User-visible behavior, state changes, and failure handling.
4. Responsive behavior, keyboard operation, and accessibility.
5. Public test boundaries, acceptance criteria, and real App checks.
6. Implementation locations, known remaining work, and release requirements.

Use this implementation order:

1. Write the design document.
2. Add public behavior tests.
3. Implement the change.
4. Run the full tests and build.
5. Update the local desktop App and inspect the real interface.
6. Record the implementation status, test result, and remaining work in the design document.
7. Create a local commit with an English commit message.

For an urgent fix, write a small design record before you change the code. Include the symptom, expected
behavior, safety boundaries, and regression test.

## Update the local desktop App

Update the local Token Station App before you deliver a change to executable behavior or the interface.
This requirement applies to source and build configuration under `apps/`, `crates/`, and `plugins/`.

Run this command:

```bash
scripts/install-local-desktop.sh
```

The script must use this order:

1. Build and audit the new App with `scripts/build-desktop.sh --local`.
2. Exit and remove the old App only after the new App passes all checks.
3. Replace only `/Applications/token-station.app` with bundle ID `com.tokenstation.desktop`.
4. Verify the bundle ID and code signature after installation.
5. Start the App and inspect it.

Do not use a wildcard to remove or replace an App. Keep the installed App if the new build fails. Report
each failed step accurately.

A change to documentation, comments, or test data does not require App installation. Install the App if
the user explicitly requests it.

## Publish an unsigned cross-platform preview release

Use this procedure when a preview includes Windows or Linux packages without formal platform signing
credentials. Publish it as a GitHub pre-release. Do not mark it as a stable or formal release.

1. Prepare one aligned SemVer release commit on `main`. Update every file checked by
   `scripts/check-release-readiness.mjs`. Add `docs/release/vX.Y.Z.md` with explicit unsigned-package warnings.
2. Push the release commit. Wait for the exact commit's Full CI run to pass.
3. Dispatch `.github/workflows/preview-platform-artifacts.yml` on that exact commit. Wait for Full CI
   verification, Platform Gates, the Windows MSI build, and all Linux package builds to pass.
4. Build the two macOS preview targets on the authorized offline updater-signing host:

```bash
scripts/build-desktop.sh --preview --target aarch64-apple-darwin
scripts/build-desktop.sh --preview --target x86_64-apple-darwin
```

5. Assemble the two unsigned DMGs and checksum files, two updater payloads and signatures, Windows MSI,
   Linux AppImage, Debian, and RPM packages, and `latest.json` in one new empty directory.
6. Create `SHA256SUMS`. Run `scripts/check-preview-release-assets.mjs` against the assembled directory.
7. Create an annotated `preview-vX.Y.Z` tag on the release commit. Push the tag only after every package
   audit passes. This tag must not match the formal `v*` release trigger.
8. Create a GitHub pre-release named `Token Station vX.Y.Z`. Upload the exact verified directory. Do not
   overwrite an asset in a published versioned preview.
9. Replace only `updater-preview/latest.json` after every versioned asset URL, updater signature, and checksum
   passes a fresh download check.

Windows and Linux updates remain manual. A Windows preview MSI can show a SmartScreen warning. macOS preview
Apps are ad-hoc signed, Apple-unsigned, and unnotarized. The formal release workflow and its signing
requirements do not change.

## Publish an Apple Silicon updater preview release

Use this procedure only for an Apple Silicon-only preview. This procedure publishes an ad-hoc signed,
Apple-unsigned, and unnotarized Apple Silicon App. Do not call this a stable or formal release.

Publishing only a versioned GitHub Release does not update an installed App. The versioned release stores
immutable files. The `updater-preview` release stores the rolling `latest.json` file that installed Apps
check. Publish the versioned release first. Replace the rolling manifest only after every check passes.

Keep the updater private key offline. Do not add it to this repository or GitHub Actions. Do not print its
contents or password. Use the existing authorized local key, public key, and Keychain password. Stop if any
of these credentials are missing. Do not generate a replacement key during a release.

### 1. Prepare the release commit

1. Confirm that the branch is `main`.
2. Confirm that the working tree has no unrelated changes.
3. Download the current rolling manifest from the fixed URL in step 2 to a new temporary directory.
   Parse its `version` with `jq`; stop if the download or parse fails. Do not rely on a repository file.
4. Choose a SemVer version greater than both the rolling manifest version and every existing version tag.
5. Update the version in every file checked by `scripts/check-release-readiness.mjs`.
6. Update both Cargo lockfiles and the desktop npm lockfile.
7. Add `docs/release/vX.Y.Z.md` with concise English release notes.
8. Keep the notes to the changes, update behavior, installation, and the unsigned warning.
9. Do not add another visible file to the DMG.
10. Keep only `token-station.app`, `Applications`, and `README.md` visible in the DMG.
11. Run the release and DMG policy checks.

```bash
node scripts/check-release-readiness.mjs --version "$VERSION"
node scripts/check-macos-dmg-packaging.mjs
tests/build-desktop-verbosity.sh
tests/desktop-updater-release.sh
```

Run the complete Rust, desktop Rust, and frontend checks from `.github/workflows/ci.yml`. Build the frontend.
Commit the version and release-note changes directly to `main`. Create an annotated `vX.Y.Z` tag on that
exact commit. Do not move an existing version tag.

### 2. Load the preview signing configuration

Use this fixed updater URL:

```text
https://github.com/ballast-ai/token-station/releases/download/updater-preview/latest.json
```

Load these environment variables without displaying their values:

- `TOKEN_STATION_UPDATER_ENDPOINT`: The fixed URL above.
- `TOKEN_STATION_UPDATER_PUBKEY`: The existing Tauri updater public key.
- `TAURI_SIGNING_PRIVATE_KEY_PATH`: The existing offline updater private-key file.
- `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`: The password from macOS Keychain.

The authorized local key is under `~/.config/token-station/release/`. The Keychain service is
`com.tokenstation.updater-signing`. Read the public-key file as one value with whitespace removed. Unset the
private-key password after the build. `scripts/build-desktop.sh` must remove private signing material from
the dependency compilation environment and expose it only to the bundle-only signing phase.

### 3. Build from the tagged commit

Confirm that `HEAD` is the new version tag. Run the preview build on an Apple Silicon Mac:

```bash
scripts/build-desktop.sh --preview --target aarch64-apple-darwin
```

The build must create and audit these source artifacts:

- `bundle/dmg/token-station_X.Y.Z_aarch64_UNSIGNED-UNNOTARIZED.dmg`
- `bundle/dmg/token-station_X.Y.Z_aarch64_UNSIGNED-UNNOTARIZED.dmg.sha256`
- `bundle/macos/token-station.app.tar.gz`
- `bundle/macos/token-station.app.tar.gz.sig`

Copy the updater payload and signature into a new empty release directory. Rename them to
`token-station_X.Y.Z_aarch64.app.tar.gz` and `token-station_X.Y.Z_aarch64.app.tar.gz.sig`. Do not modify either
file after signing.

### 4. Create and verify the update manifest

Create `latest.json` with `scripts/create-desktop-update-manifest.mjs`. Use these inputs:

- The new version without the `v` prefix.
- The current UTC time in RFC 3339 format.
- `https://github.com/ballast-ai/token-station/releases/download/vX.Y.Z` as the release base URL.
- `darwin-aarch64` as the only platform.
- The renamed `.app.tar.gz` file as the updater artifact.
- The concise English release-note file as the notes file.

Verify the renamed updater payload with `ts-release verify-updater` and the trusted updater public key.
Verify the DMG checksum. Run `scripts/audit-macos-dmg.sh --unsigned-test` against the final DMG. Inspect the
mounted DMG in Finder. Confirm that it shows one App, one Applications link, and one README.

### 5. Push and publish the versioned release

Push the release commit to `origin/main`. Push the annotated version tag. Wait for the CI and macOS platform
checks for the exact commit. Do not publish if a required check fails or is still running.

Create one GitHub pre-release named `Token Station vX.Y.Z`. Upload only these five files:

1. `token-station_X.Y.Z_aarch64_UNSIGNED-UNNOTARIZED.dmg`
2. `token-station_X.Y.Z_aarch64_UNSIGNED-UNNOTARIZED.dmg.sha256`
3. `token-station_X.Y.Z_aarch64.app.tar.gz`
4. `token-station_X.Y.Z_aarch64.app.tar.gz.sig`
5. `latest.json`

Use the concise English note file for the GitHub release body. Never overwrite an asset in a published
versioned release. Publish a higher version if an uploaded file is wrong.

### 6. Enable the in-App update

Download and save the current `updater-preview/latest.json` before changing it. Confirm that every URL in the
new manifest downloads from the new versioned release. Then replace only `latest.json` on the existing
`updater-preview` pre-release. Do not delete or recreate the rolling release.

Download the rolling manifest again. Confirm its version, signature, platform, and versioned asset URL.
Download the updater payload from that URL. Verify it again with the trusted updater public key.

### 7. Test the real update and report the release

Keep the previous public App installed until this test. Open that App and select **Check for Updates**. Confirm
the update. Verify that the App downloads the payload, verifies it, installs it, restarts, and reports the new
version. This confirmation is required. The updater must not install without user confirmation.

After the real update passes, report these links and facts:

- The versioned GitHub pre-release URL.
- The direct DMG URL.
- The rolling `latest.json` URL.
- The exact release commit and tag.
- The CI and macOS check results.
- The real previous-version to new-version update result.
- The Apple Silicon, ad-hoc signed, Apple-unsigned, and unnotarized limitations.

If the real update fails, stop the release. Restore the saved rolling manifest if it was already replaced.
Do not change or delete the versioned assets. Fix the problem and publish a higher version.

This procedure does not grant permission to publish a release. Publish only when the user explicitly requests
it.
