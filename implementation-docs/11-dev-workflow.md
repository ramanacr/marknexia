# 11 — Development Workflow

## Development environment

Recommended:

- Visual Studio 2022/next supported version for the selected Windows App SDK baseline.
- .NET SDK matching the project target.
- Windows 10/11 SDK matching supported OS targets.
- WebView2 runtime available.
- Git.

The exact SDK/package versions should be pinned in the repository once implementation starts and upgraded deliberately.

## Build configuration

```text
Debug
Release
Store
```

`Store` can be a Release-equivalent configuration with packaging/signing metadata appropriate for Store CI.

## CI stages

```text
Restore
  ↓
Build
  ↓
Unit tests
  ↓
Static analysis
  ↓
Security tests
  ↓
Rendering snapshots
  ↓
UI smoke tests
  ↓
Package MSIX
  ↓
Package validation
  ↓
Artifact publish
```

## Code quality

Enable:

- Nullable reference types.
- Warnings as errors for owned code where practical.
- Roslyn analyzers.
- Formatting enforcement.
- Dependency vulnerability checks.
- License checks for third-party packages.

## Dependency policy

Every third-party dependency must record:

- Purpose.
- License.
- Version.
- Transitive-risk assessment.
- Offline/runtime footprint.
- Upgrade strategy.

## Branching

Use short-lived branches and merge through pull requests.

Required checks before merge:

- Build.
- Unit tests.
- Security tests.
- Relevant rendering/navigation snapshots.

## Commit strategy

Use small, reviewable commits with a change category prefix, e.g.:

```text
feat(rendering): add GFM alert blocks
fix(nav): resolve cross-file fragments
security(html): block unsafe URI schemes
perf(render): defer Mermaid startup
```

## Agent-friendly implementation

Every feature implementation should begin by updating the relevant design/acceptance document if behavior changes.

Agents should not invent alternate architecture in an isolated feature branch without recording an ADR or updating the design contract.
