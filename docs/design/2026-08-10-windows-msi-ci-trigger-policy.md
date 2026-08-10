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

- Run the MSI lifecycle on pushes to `main`.
- Run the MSI lifecycle when a maintainer starts CI with `workflow_dispatch`.
- Skip the MSI lifecycle for all pull requests, including desktop changes.

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

For a pull request, `Windows x64 MSI real lifecycle` is shown as skipped. Other PR
checks continue normally. On `main` or a manual run, an MSI lifecycle failure
continues to fail CI and retains its diagnostic artifacts.

## Accessibility and interaction

This change has no application UI, keyboard, responsive-layout, or accessibility
impact. Maintainers use the existing GitHub Actions manual-run control.

## Public test boundary and acceptance criteria

- A pull-request event does not satisfy the `windows-msi` job condition.
- A push whose ref is `refs/heads/main` satisfies the event portion of the
  condition.
- A `workflow_dispatch` event satisfies the event portion of the condition.
- The existing changed-path output and prerequisite jobs still gate actual MSI
  execution.
- All non-MSI PR jobs remain unchanged.

## Implementation and release requirements

The implementation changes only the `windows-msi` condition in
`.github/workflows/ci.yml`. No local desktop installation is required because the
change affects hosted CI scheduling rather than application behavior. A future
Windows release must still pass the full lifecycle through `main` or a manual run.

## Implementation status

Implemented. The MSI lifecycle no longer runs for pull requests and remains
enabled for `main` and manually dispatched CI runs.
