/**
 * Mirrors the Rust `KeyringHint` enum from
 * `src-tauri/src/keyring.rs`. The serde shape is
 * `{ kind: "empty" }` / `{ kind: "public_only" }` /
 * `{ kind: "key_pair" }` / `{ kind: "unavailable", reason: "..." }`
 * — see the `#[serde(tag = "kind", rename_all = "snake_case")]`
 * on the Rust side. We hand-roll the TS type (rather than
 * importing a generated one) because the schema is small +
 * stable — adding a generated schema fetch would be more
 * machinery than the value it carries.
 *
 * Lives in `$lib/api/` so the Svelte component
 * (`SshKeySettings.svelte`) and the test file
 * (`SshKeySettings.test.ts`) can both
 * `import { type KeyringHint } from "$lib/api/keyring"`
 * without round-tripping through the `.svelte` module —
 * which `tsc --noEmit` (no `svelte:registry` plugin) treats
 * as a default-export-only module.
 */
export type KeyringHint =
  | { kind: "empty" }
  | { kind: "public_only" }
  | { kind: "key_pair" }
  | { kind: "unavailable"; reason: string };
