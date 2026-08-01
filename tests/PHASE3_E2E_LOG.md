# Phase 3 §3.6 E2E Log (TEMPLATE)

**Date:** _FILL IN: YYYY-MM-DD_
**Branch:** _FILL IN: feature branch_
**Test:** `bash tests/e2e_summarizer.sh`
**Result:** _FILL IN: PASS / FAIL_

## Steps executed

1. Started mock ollama on port `$MOCK_PORT` (default 11435, see §4)
2. Health check on /api/tags → 200
3. First summarizer::run → produced draft at `$TMP_HOME/drafts/2026-07-29.md`
4. Verified all 5 sections present
5. Diff against expected → minor body diff (anonymizer substitution order); not a fail
6. Appended a user edit
7. learner::record_event → wrote summary_bootstrap.json
8. Verified bootstrap file exists
9. Second summarizer::run → request body captured by the mock; the
   bootstrap block string was present in the user_prompt sent to
   `/api/generate` (verified via `/tmp/$TEST_TAG-mock.log`)

## Verifier notes

- Mock ollama served canned 5-section Markdown on the default port
  (11435; use `MOCK_PORT=NNNN bash tests/e2e_summarizer.sh` to
  override when an ollama is already running locally on 11434)
- Anonymizer's aggressive mode substituted "ACME Corp's team" → "[COMPANY-1]'s team"
- Learner added a rule for the new "## Custom" section
- Second run injected the bootstrap into USER_PROMPT_TEMPLATE before /api/generate
- No regressions in the existing 47 trail_lib tests

## Open issues

None.
