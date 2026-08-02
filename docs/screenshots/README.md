# Screenshots

Placeholder PNGs committed so the README's `![Alt](docs/screenshots/*.png)`
links don't 404 on first visit.

| File                       | What it shows (when real)                           |
|----------------------------|-----------------------------------------------------|
| `menu-bar.png`             | The macOS menu-bar popover with summary + actions   |
| `review-window.png`        | The Review window showing the draft DaySummary      |

The placeholder images are 800x500 / 800x600 dark backgrounds with a
"placeholder — replace with real screenshot" caption. They are
deliberately low-information — they exist only to keep the README's
image references valid until Pedro runs `tests/PHASE7_VERIFICATION.md`'s
final §7.9 visual step and replaces them with the actual screenshots
from his macOS M5 Max.

Visual verification of the menu-bar popover + Review window is a
Pedro action, not a `cargo test` or `act -n` gate. Replacing these
placeholders is the only place the README hinges on visual review.
