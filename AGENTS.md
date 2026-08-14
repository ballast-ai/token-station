# Token Station Engineering Rules

## Use English by default

Use English for new and modified technical content. This rule applies to code comments, documentation,
design records, test names, commit messages, pull requests, release notes, and contributor instructions.

Keep commands, paths, protocol fields, API names, and code identifiers unchanged. Keep required localized
user-interface text in its target language. You can quote an error or log entry in its original language.

If a document must contain English and Simplified Chinese, put the complete English text first. Put the
complete Simplified Chinese text after it.

Write commit messages in English. Use a short imperative subject. Describe one logical change in each
commit.

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

Create or update a file in `docs/design/` before you change any of these items:

- User-visible interfaces.
- Interaction flows.
- State models.
- Frontend and backend contracts.
- Release behavior.

Keep the design document and its implementation in the same pull request. The review and maintenance
process must not depend on access to a private repository.

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

## Read the macOS DMG requirements

Before you create, change, upload, or release a macOS DMG, read and follow
[`docs/release/macOS-DMG安装提示与打包要求.md`](docs/release/macOS-DMG安装提示与打包要求.md).

This requirement does not grant permission to create or publish a release.
