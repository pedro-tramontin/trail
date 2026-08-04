"""
label-rules.py -- the single source of truth for the
title -> type: <name> mapping in Trail's release pipeline.

Both .github/workflows/labeler.yml and the pr-label-check job
in .github/workflows/draft-build.yml import this module:

    sys.path.insert(0, '.github')
    import label_rules

RULES is an ordered list. Order matters: `feat!:` must be
tested BEFORE `feat:` so a breaking-change title buckets
into `type: breaking`, not `type: feature`. The first rule
whose `title_regex` matches the PR title wins.

The regexes follow Conventional Commits v1.0.0:
    <type>[(scope)]!: <description>

The scope is optional and may contain any non-`)` character.
The breaking-change `!` suffix only matters for the
`type: breaking` rule; other rules accept `!` too but the
ordering ensures breaking wins.

When you add a rule, also add a row to labeler.yml's
color palette (the script that creates the label on the
repo with a hand-picked color) and to the pr-label-check
job's expected-prefixes error message.
"""

# Format: (label, regex)
# `regex` is a Python `re` pattern (NOT a JSON string), so
# backslashes and escapes are written naturally.
RULES = [
    ("type: breaking", r"^(?:feat|fix|refactor|chore|ci|build|perf|style|docs|test)(?:\([^)]+\))?!\s*:"),
    ("type: feature",  r"^feat(?:\([^)]+\))?\s*:"),
    ("type: bug",      r"^fix(?:\([^)]+\))?\s*:"),
    ("type: refactor", r"^refactor(?:\([^)]+\))?\s*:"),
    ("type: perf",     r"^perf(?:\([^)]+\))?\s*:"),
    ("type: docs",     r"^docs(?:\([^)]+\))?\s*:"),
    ("type: chore",    r"^chore(?:\([^)]+\))?\s*:"),
    ("type: ci",       r"^ci(?:\([^)]+\))?\s*:"),
    ("type: test",     r"^test(?:\([^)]+\))?\s*:"),
    ("type: build",    r"^build(?:\([^)]+\))?\s*:"),
    ("type: style",    r"^style(?:\([^)]+\))?\s*:"),
]


def match(title):
    """Return the matching label for `title`, or None if no rule matches."""
    import re
    for label, pattern in RULES:
        if re.match(pattern, title):
            return label
    return None


def all_prefixes():
    """Human-readable list of expected prefixes, for error messages."""
    return [label.replace("type: ", "") + ":" for label, _ in RULES]
