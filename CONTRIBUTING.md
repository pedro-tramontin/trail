# Contributing to Trail

Thanks for your interest in contributing. Trail is a single-maintainer solo project
right now, so the contribution process is intentionally lightweight — but the
technical bar is the same as a larger project.

## Before you start

- **Open an issue first** for non-trivial changes. A change is non-trivial if it's
  > 100 LOC altered (excluding tests), or changes user-facing behavior, or adds a
  new dependency. Use the issue to confirm the design fits the project direction.
- **For bug fixes and small improvements** (typos, missing test cases, refactors
  with no behavior change), skip the issue and open a PR directly.
- **For new Tauri commands, new collectors, or new transport variants**, the issue
  is mandatory — these touch the public IPC surface and the type system. The
  design discussion helps avoid wasted work.

## Development setup

See [`docs/developer.md`](docs/developer.md) for the full setup: toolchain,
building, running tests, and the inner dev loop. The TL;DR is `make lint && make
test` runs the same pipeline CI runs.

## Coding style

The style is enforced by `cargo fmt`, `cargo clippy --all-targets -- -D warnings`,
and `pnpm test`. There is no separate style guide. Run them before opening a PR:

```bash
make fmt      # cargo fmt --all -- --check
make lint     # cargo clippy --workspace --all-targets -- -D warnings
pnpm test     # vitest
```

When you add a new lint suppression (a `#[allow(...)]` or an `eslint-disable`),
leave a one-line `// reason:` comment. Reviewers push back on blanket suppressions.

## Tests

- All new code should have tests. The CI `release.yml` job runs the test suite; if
  it's red, the PR is blocked.
- For Rust: put unit tests in the same file as the code (the `mod tests { ... }`
  pattern). The test count is documented in
  [`tests/PHASE7_VERIFICATION.md`](../tests/PHASE7_VERIFICATION.md); the bar is
  that count only goes up.
- For UI: put vitest cases in a `*.test.ts` sibling to the source file. The bar is
  the same — the vitest count only goes up.
- For the e2e test (`tests/e2e_collector.sh`): the script defaults to skip mode
  on hosts that can't reach the VPS. Re-run it on a real macOS laptop when the
  architecture changes — see [`docs/e2e-runbook.md`](docs/e2e-runbook.md) for the
  operator guide.

## Pull request process

1. Branch off `main` with a descriptive name: `fix/<thing>`, `feat/<thing>`,
   `docs/<thing>`, `chore/<thing>`. The `infinite-loop-dev` skill (when used by
   the maintainer's agent) uses `feat/<n>-<slug>` and `fix/<n>-<slug>`.
2. Commit messages follow the [Conventional Commits](https://www.conventionalcommits.org/)
   spec — `feat:`, `fix:`, `docs:`, `chore:`, `refactor:`, `test:`, `build:`,
   `ci:`. The `feat:` and `fix:` prefixes trigger a release-please PR on the next
   push to `main`.
3. The PR description should explain **what** changed and **why** (not just the
   diff). If the change is a deviation from a documented plan, call that out in
   the PR body.
4. All CI jobs must be green: `release-please`, `draft-build`, `release`. If a
   CI job is failing for an unrelated reason, fix the unrelated thing or wait for
   it to recover — don't merge with a red check.
5. **Do not push directly to `main`.** Always go through a PR. Direct pushes
   bypass the 5-gate check.
6. The maintainer merges PRs. For trusted contributors' PRs, the maintainer may
   enable `auto-merge` on the PR after review; for everyone else, the maintainer
   does the squash-merge manually.

## Design documents

Design decisions live in [`docs/`](docs/). The currently published ones:

- [`docs/architecture.md`](docs/architecture.md) — the canonical design overview
  (laptop + VPS topology, data flow, transport trait, invariants).
- [`docs/developer.md`](docs/developer.md) — the hands-on dev guide.
- [`docs/security.md`](docs/security.md) — the threat-model controls and the
  supply-chain policy.
- [`docs/e2e-runbook.md`](docs/e2e-runbook.md) — the e2e SSH push + collector
  `--once` test against a real VPS.
- [`docs/screenshots/`](docs/screenshots/README.md) — the README for the bundled
  screenshots used in the top-level README.

If your change is non-trivial, consider whether one of these needs an update. The
bar for adding a new doc is "will future-me wonder why we did it this way?" If
yes, write it.

## Release process

Trail uses [release-please](https://github.com/googleapis/release-please) for
releases. You do not need to cut a release manually — every `feat:` or `fix:` PR
that lands on `main` triggers a release-please PR, which the maintainer merges
when ready. The release is cut by the tag push, and the `release.yml` workflow
builds the binaries (macOS only in v1).

If a release-please PR fails to build, the `release.yml` workflow has detailed
logs in the Actions tab. The common culprits are:

- A new GitHub Action reference that's not SHA-pinned (the supply-chain policy
  requires pinning).
- A new `cargo` dependency that pulls in a transitive advisory (the
  `cargo deny` check is the gate).
- A `pnpm` advisory at moderate severity or above (the `pnpm audit` check is the
  gate).

## Security

If you find a security issue, **do not open a public issue**. Email the
maintainer directly (see the GitHub profile). For non-security bugs, open a
public issue.

The threat-model controls and the supply-chain policy are documented at
[`docs/security.md`](docs/security.md). If you're adding a new collector, a new
transport, or a new persistence path, walk through the security checklist at
the bottom of that doc.

## AI policy

Trail is a small project maintained by one person, with help from an AI coding
agent (Hermes). AI assistance is welcome for:

- **Writing tests.** Vitest / `cargo test` are deterministic; AI-generated tests
  are fine as long as you can explain what each one is checking.
- **Boilerplate.** Type definitions, Tauri command skeletons, doc templates.
- **Review.** AI code review (e.g. GitHub Copilot) is encouraged — Copilot
  catches real bugs (see the §1.3 PR review on `transport.rs` for a load-bearing
  example).

AI assistance is **not** welcome for:

- **PRs you don't understand.** If you can't explain what the change does, don't
  submit it.
- **Comments that only help the AI interact with the code.** Comments that
  explain what straightforward code does are not useful and should be removed.
- **Drive-by AI reviews on other people's PRs** without the intention to follow
  up. If you invoke an AI review, be ready to address its findings.

If you used AI to write part or all of a PR, say so in the PR description ("This
PR was written with help from [tool]. I reviewed every line and the [X] test
that I can't explain is the part I'm least sure about."). Transparency helps
the reviewer calibrate.

## License

By contributing, you agree that your contributions will be licensed under the
[Apache-2.0](LICENSE) license, the same as the rest of the project.

## Communication

- **GitHub issues** for bugs, feature requests, and design discussion.
- **GitHub PRs** for code review.
- The maintainer is one person. Expect a few days of latency on issue triage.
