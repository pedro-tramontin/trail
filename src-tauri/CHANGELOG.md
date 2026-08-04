# Changelog — Trail (src-tauri)

All notable changes to the Trail desktop app (src-tauri) are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.1] - 2026-08-04

- No user-facing changes in this release.

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
