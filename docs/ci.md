# Continuous integration (CI)

The two main CI workflows are adaptive. On a pull request, they run only the jobs for the
parts of the repository that the pull request changes. All other events run all jobs.

- `.github/workflows/ci.yml` ("CI & Quality Assurance"): the .NET edition and the bridge tests.
- `.github/workflows/rust-feasibility.yml` ("Rust Feasibility"): the Rust edition and the
  .NET release preservation suite.

The other workflows do not change. `pages.yml` keeps its path filter. `release.yml`,
`rust-preview-release.yml` and `signed-packages.yml` run on tags or by hand, always in full,
with no cache.

## How a run decides what to do

1. The `changes` job ("Detect changed areas") runs first, on Ubuntu.
2. It runs the classifier tests (`tests/ci/changed_areas.tests.ps1`).
3. It runs `scripts/ci/Get-ChangedAreas.ps1`:
   - On `pull_request`, it reads `git diff --name-only --no-renames HEAD^1 HEAD`. The checkout
     is the pull request merge commit, so `HEAD^1` is the tip of `main`.
   - On `push`, `merge_group`, `schedule` and `workflow_dispatch`, it selects all jobs.
   - If `git diff` fails, or finds no files, it selects all jobs.
4. Each changed path gets one area (the first rule that matches, in
   `scripts/ci/ChangedAreas.psm1`). Each area selects job groups.
5. Each job runs only when its group is selected. A job that is not selected shows as
   "skipped".
6. The gate job runs last and reports the result.

The step summary of the `changes` job shows the reason and the area of each file.

## Areas

| Area | Paths | Job groups |
| --- | --- | --- |
| ci | `.github/**`, `scripts/ci/**`, `tests/ci/**` | all |
| repo | `.gitattributes`, `.gitignore` | all |
| unknown | any path that no rule matches | all |
| web-assets | `src/Marknexia.Rendering/Assets/**` | dotnet, bridge, rust |
| bridge | `tests/Marknexia.Bridge.Tests/**` | bridge |
| compat | `compat/**` (parity fixtures and decision records) | dotnet, rust |
| fixtures | `test-fixtures/**` | dotnet, rust |
| dotnet | `src/**`, `tests/Marknexia.*.Tests/**`, `tools/Marknexia.ParityExporter/**`, `*.slnx`, `Directory.Build.props`, `Directory.Packages.props`, `global.json`, `nuget.config`, the .NET scripts (`Build-App`, `Test-AppOutput`, `Generate-Sbom`, `Test-Sbom`, `set-version`, `Generate-MarkdownFileIcons`, `Test-UiContracts`) | dotnet |
| rust | `crates/**`, `fuzz/**`, `Cargo.toml`, `Cargo.lock`, `.cargo/**`, `rust-toolchain*`, `deny.toml`, `tools/rust-tools.lock`, `tests/rust/**`, `tests/perf/**`, `scripts/*Rust*.ps1`, `scripts/Stage-WebView2Loader.ps1`, `LICENSE` | rust |
| packaging | `packaging/**`, `store-submission/**`, `scripts/signing/**`, `tests/signing/**`, the signing and package scripts (`Invoke-ReleaseSigning`, `Test-SignedHandoff`, `prepare-store-package`, `Build-Installer`, `Test-Installer`, `Test-MsixPackage`) | rust |
| docs | `docs/**`, `branding-docs/**`, `implementation-docs/**`, `*.md` at the repository root | none |

Why some paths are in an unexpected area:

- `compat/**` runs both editions. The Rust parity tests read the fixtures and every
  `compat/decisions/*.md` file (`crates/marknexia-markdown/tests/parity_v1.rs`). The .NET
  parity tests read `compat/fixtures/v1`.
- `test-fixtures/**` runs both editions. The .NET navigation tests read it, and the Rust
  release build records the digest of `test-fixtures/markdown/gfm/features.md`.
- `src/Marknexia.Rendering/Assets/**` runs all groups. The Rust build embeds
  `mermaid.min.js` from it (`crates/marknexia-webview/build.rs`), and the Rust and bridge
  tests read the other assets.
- `LICENSE` runs Rust because `scripts/Generate-RustSbom.ps1` reads it.
- `store-submission/**` runs Rust because `scripts/New-RustInstallerPackages.ps1` reads it.
- A Markdown file outside the docs paths follows its directory. For example,
  `compat/decisions/*.md` is `compat`, and `packaging/webview/README.md` is `packaging`.

## What runs for each group

| Group | `ci.yml` jobs | `rust-feasibility.yml` jobs |
| --- | --- | --- |
| dotnet | Build & Test (x64 + ARM64) | .NET Release preservation |
| bridge | Rendered document bridge (Playwright) | none |
| rust | none | Rust quality and policy, Rust release build (x64, ARM64), Native gates (x64, ARM64), Fuzz |

Examples for a pull request:

- Only `docs/**` or root `*.md` changes: no heavy job runs. Both gate jobs pass.
- Only `crates/**` changes: only the Rust jobs run.
- Only `src/Marknexia.Core/**` changes: Build & Test and .NET Release preservation run.
- A `compat/decisions/*.md` change: the .NET and Rust jobs run.
- Any `.github/**` change, or an unknown path: all jobs run.

## Gate jobs

Each workflow has one gate job. It always runs (`if: always()`) and needs every other job.

| Workflow | Gate job (check name) |
| --- | --- |
| `ci.yml` | `CI result` |
| `rust-feasibility.yml` | `Rust CI result` |

A gate job fails when the `changes` job did not succeed, or when a needed job failed or was
cancelled. It passes when each needed job succeeded or was skipped by design.

### Branch protection

Require only these two checks on `main`:

1. `CI result`
2. `Rust CI result`

Do not require the individual jobs. A skipped job never reports, so a required individual job
would stay "pending" on a pull request that does not change its area.

## Concurrency

- Pull request: a new push cancels the running workflow for the same pull request.
- Push, merge queue, schedule and manual run: each run has its own concurrency group (the run
  ID). These runs are never cancelled, and a pending run is never replaced by a newer one.
  `signed-packages.yml` needs a green push run of "Rust Feasibility" on `main`.

## Nightly and merge queue runs

- Both workflows run in full every night on `main` (`schedule`: 02:23 UTC for `ci.yml`,
  02:43 UTC for `rust-feasibility.yml`). This finds problems that a skipped job did not show.
- Both workflows have the `merge_group` trigger and run in full for the merge queue.

## Force a full run

- Actions tab: select the workflow, then **Run workflow**, then select the branch. A manual run
  always runs all jobs and reports its checks on that commit.
- Or change a file under `.github/` or `scripts/ci/` in the pull request.

## Caches

Caches only save download and compile time. They do not change what is built. Caches are
saved only from `main`. Pull requests and the merge queue only restore them.

| Job | Cache | Key |
| --- | --- | --- |
| Build & Test, .NET Release preservation | NuGet packages (`~/.nuget/packages`) | OS, architecture, hash of every `*.csproj`, `Directory.Packages.props`, `Directory.Build.props` |
| Rendered document bridge | npm (`actions/setup-node`, unchanged) | `tests/Marknexia.Bridge.Tests/package-lock.json` |
| Rust quality and policy | `Swatinem/rust-cache`: registry, `~/.cargo/bin`, compiled dependencies | job, rustc version, Cargo and toolchain files |
| Native gates | `Swatinem/rust-cache` | as above, plus the target triple |
| Fuzz | `Swatinem/rust-cache` for `fuzz/target` | as above; a new nightly starts a new cache |
| Rust release build | Cargo registry downloads only (never a `target` directory) | OS, target triple, hash of `Cargo.lock` |

The Rust release build compiles both reproducibility copies from clean target directories, as
before. Cargo checks each cached crate against `Cargo.lock` (`--locked`).

## Change the rules

1. Edit the rules in `scripts/ci/ChangedAreas.psm1`.
2. Add a case to `tests/ci/changed_areas.tests.ps1`.
3. Run `pwsh -NoProfile -File tests/ci/changed_areas.tests.ps1`.
4. Check a list of paths:
   `pwsh -NoProfile -Command "./scripts/ci/Get-ChangedAreas.ps1 -Path docs/ci.md, crates/marknexia-core/src/lib.rs"`.

When a test or build starts to read a new path, add a rule that maps that path to the job
group that reads it. A path with no rule runs all jobs.
