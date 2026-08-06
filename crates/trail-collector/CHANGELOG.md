# Changelog — trail-collector
#
# All notable changes to the trail-collector binary
# (the VPS-side collector) are documented in this file.
# The version is bumped in lockstep with src-tauri per
# the project's standing rule.
#
# The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
# and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).


## [0.4.0] - 2026-08-05

### Added

- feat(onboarding): 10s scan countdown + 18:00 local review time (#141)
- feat(onboarding): editable review time + why-disabled tooltips + drop Resume (#143)

### Fixed

- fix(ci): add `trail-` prefix to release-drafter changelog compare link (#139)
- fix(onboarding): fixed-height wizard + scrollable step body + sticky bottom controls (#144)
- fix(ci): drop broken "Full Changelog" link from release-drafter template (#145)
- fix(onboarding): stable-width card layout, inline edits, HH:MM review time (#146)

## [0.3.0] - 2026-08-05

### Added

- feat(release): attach debug binaries to draft release on every push (Talon-style draft pipeline) (#124)

### Fixed

- fix(rust): cfg(unix) load_private_key_pem to silence Windows dead-code warning (#123)
- fix(onboarding): support Platform::Other on Windows + center wizard in window (#126)
- fix(ci): remove non-existent --draft flag from attach-debug-to-draft.yml (#128)
- fix(ci): add actions/checkout@v4 to attach-debug gate (gh needs cwd) (#129)
- fix(ci): drop skip-when-version-bump-PR-open from attach-debug gate (#130)
- fix(ci): correct macOS step order + add includeDebug to tauri-action (#131)
- fix(ci): drop duplicate --debug from tauri-action args (#132)
- fix(ci): use GITHUB_TOKEN (uppercase) for tauri-action in attach-debug (#133)
- fix(ci): codesign should find .app under target/ not src-tauri/target/ (#134)
- fix(ci): make macOS codesign step non-fatal (no identity on runner) (#135)
- fix(ci): close superseded version-bump PRs when draft version changes (#136)
- fix(ci): fetch orphan branch tip before --force-with-lease push (#137)
- fix(ci): no-op version-bump when a same-version PR is already open (#138)

## [0.2.0] - 2026-08-04

### Changed

- chore(release): bump version to 0.2.0 (#121)
- docs(arch): update to 'tray-icon app with auto-pop windows', fix release-drafter ref (#119)

### Added

- feat(ui): open onboarding window on first launch, wire start_collectors (#116)
- feat(ui): build tray icon at startup, remove dead §5.7 scaffold (#114)

### Fixed

- fix(ci): race-aware pr-label-check (retry loop for labeler concurrency) (#122)
- fix(ci): --force-with-lease push in version-bump so stale orphans self-heal (#120)
- fix(ci): ad-hoc codesign macOS bundle in promote.yml + release.yml (#117)
- fix(ci): switch version-bump to RELEASE_PLEASE_TOKEN (unblocks PR creation) (#115)
- fix(boot): survive missing config on first launch (#113)

## [0.1.1] - 2026-08-04

### Changed

- chore(release): bump version to 0.1.1 (#107)
- fix(ci): labeler NameError on `sys.argv` without `import sys` (#108)

### Fixed

- fix(ci): promote gate regex matches GitHub merge commit subject (#112)
- fix(ci): gate subsequent steps on already_at output (#111)
- fix(ci): version-bump skip when main already at draft version (#110)
- fix(ci): word-split bug in stale-label removal loop (#109)

## [0.1.0] - 2026-08-03

### Changed

- Migrated release pipeline from release-please to release-drafter
  + version-bump + promote. Releases are now driven by PR title
  prefix (`feat:` → minor, `fix:` → patch, `feat!:` → major) and a
  manual version-bump PR gate before promote.yml builds + publishes.
- Reset version files to 0.1.0 as the foundation of the new pipeline.

### Removed

- release-please configuration, manifest, and workflow.
- Stale `CHANGELOG.md` content from the previous release-please
  history (5 broken releases: trail-v0.3.0, trail-v0.4.0, trail-v0.4.2,
  trail-v0.4.4, trail-collector-v0.1.1).

[0.1.0]: https://github.com/pedro-tramontin/trail/releases/tag/trail-v0.1.0
[0.1.1]: https://github.com/pedro-tramontin/trail/releases/tag/trail-v0.1.1
[0.2.0]: https://github.com/pedro-tramontin/trail/releases/tag/trail-v0.2.0
[0.3.0]: https://github.com/pedro-tramontin/trail/releases/tag/trail-v0.3.0
[0.4.0]: https://github.com/pedro-tramontin/trail/releases/tag/trail-v0.4.0