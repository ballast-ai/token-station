# Windows MSI CI Trigger Policy

## Problem

The Windows MSI lifecycle job builds two release installers and tests install,
launch, upgrade, downgrade rejection, and uninstall. It currently runs for every
pull request that changes the desktop application. A macOS-only Dock icon change
therefore consumes a Windows runner and waits for a release-grade installer test.

## Goal

Run the full MSI lifecycle at release boundaries instead of on ordinary pull
requests. Pull requests must continue to run the normal frontend, Rust, security,
and coverage gates.

## Scope

- Run the MSI lifecycle when a GitHub Release is published.
- Run the MSI lifecycle when a maintainer starts CI with `workflow_dispatch`.
- Skip the MSI lifecycle for ordinary pushes and all pull requests, including
  desktop changes.

## Non-goals

- Do not change MSI build, install, upgrade, downgrade, or uninstall behavior.
- Do not disable the regular Windows Rust gate after merge.
- Do not change desktop release signing or artifact publication.

## Safety and data boundaries

The lifecycle script remains unchanged and still runs on an isolated GitHub-hosted
Windows runner. The change must not install or replace an application on a local
developer machine. Release signing secrets remain restricted to the release
workflow.

## User-visible behavior and failures

For a pull request or an ordinary branch push, `Windows x64 MSI real lifecycle`
is shown as skipped. Other checks continue normally. On a published Release or a
manual run, an MSI lifecycle failure continues to fail CI and retains its
diagnostic artifacts.

The `release: published` run is explicitly post-publication validation; it does
not prevent the Release from becoming public. Maintainers who need a
pre-publication MSI gate must run the same workflow manually and wait for it to
pass before publishing.

## Accessibility and interaction

This change has no application UI, keyboard, responsive-layout, or accessibility
impact. Maintainers use the existing GitHub Actions manual-run control.

## Public test boundary and acceptance criteria

- A pull-request event does not satisfy the `windows-msi` job condition.
- A `release` event satisfies the event portion of the condition.
- A `workflow_dispatch` event satisfies the event portion of the condition.
- The existing changed-path output and prerequisite jobs still gate actual MSI
  execution.
- All non-MSI PR jobs remain unchanged.
- `scripts/check-ci-msi-trigger-policy.mjs` verifies the committed workflow
  trigger and job condition for pull-request, published-release, and manual
  events.

## Implementation and release requirements

The implementation changes the `windows-msi` condition and adds a
repository-local policy check that runs in regular CI. No local desktop
installation is required because the
change affects hosted CI scheduling rather than application behavior. A future
Windows release must still pass the full lifecycle through a published Release
event or a manual pre-publication run.

## Implementation status

Complete. `scripts/check-ci-msi-trigger-policy.mjs` rejects the previous
pull-request/main condition and passes against the committed published-release
and manual condition. The checker is part of regular CI, and the final local run
reported `Windows MSI trigger policy: PASS`. Remote execution of the full MSI
lifecycle remains intentionally limited to a published Release or maintainer
manual run.
