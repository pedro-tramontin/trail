# Phase 5 macOS verification checklist (Pedro, manual)

Run all of these on the Mac M5 Max with `cargo tauri dev` running.
Each item is a single, manually-verifiable action. Tick the box when
the action succeeds; if any item fails, file a bug + capture the
console output before continuing.

## Prerequisites

- [ ] Tailscale is connected (`tailscale status` — VPS is online).
- [ ] The whisper model is cached at `~/.trail/models/ggml-base.en.bin`
      (one-time download via §5.1; verify with
      `ls -lh ~/.trail/models/ggml-base.en.bin` → ~150 MB).
- [ ] `cargo tauri dev` is running; the tray icon is visible in the
      menu bar.
- [ ] `~/.trail/raw/` does not contain leftover entries from
      previous test runs (delete the directory if unsure).

## Mic permission (TCC)

- [ ] First launch: no TCC prompt yet (we don't capture until
      `voice_start` or the onboarding "Test mic" button).
- [ ] Trigger capture (push-to-talk or "Test mic"): TCC dialog
      appears; click **Allow**. Status reads "Authorized" after.
- [ ] Permission persists across restarts (quit Trail, relaunch,
      check status remains "Authorized").
- [ ] **Deep-link path:** System Settings → Privacy & Security →
      Microphone → toggle Trail **OFF**. In Trail, open the tray
      popover → "Open Mic Settings" item is visible. Click it →
      the deep-link opens the right System Settings pane.
- [ ] Re-enable Trail in System Settings, return to Trail, status
      flips back to "Authorized".

## Hotkey push-to-talk

- [ ] Default hotkey: **Ctrl+Shift+Space**. Press-and-hold. Icon
      goes static → active (blinks). Speak:
      *"Phase five voice test, recording marker alpha."*
- [ ] Release. Icon returns to static. Popover shows the
      transcript.
- [ ] Verify `~/.trail/raw/<date>/voice/<entry_id>.json` contains
      the spoken sentence (replace `<entry_id>` with the latest
      UUID in the directory).
- [ ] Verify the matching `.wav` plays back at correct speed +
      duration (~the length of the spoken sentence, not 5s or 0s).
- [ ] **Hotkey conflict:** rebind to a shortcut that Raycast owns
      (e.g. **Cmd+Space**). Save → "Hotkey is registered by
      another app (Raycast). Choose a different hotkey." appears.

## Stop recording

- [ ] HOLD Ctrl+Shift+Space (start). Speak. While still holding,
      tray popover → "Stop recording".
- [ ] Partial `.wav` + `.json` at `~/.trail/raw/.../voice/`
      are absent (the abort handler ran). No leaked-stream log
      error in the console.
- [ ] Re-record: HOLD Ctrl+Shift+Space again — works without
      re-prompting TCC.

## Tray blinking animation

- [ ] Start recording. Quiet room → icon blinks slowly
      (~once per 2s).
- [ ] Normal speech → blink rate doubles. Loud → fast (a few
      per second).
- [ ] Release → icon returns to static immediately (no
      lingering timer).

## End-to-end via the bash harness (optional, for the log)

- [ ] On the laptop, with the model cached: `cd ~/code/trail &&
      TRAIL_E2E_HOST=macos-laptop bash tests/e2e_voice.sh` prints
      `=== PHASE 5 E2E PASSED ===` and a populated
      `~/.trail/raw/<date>/voice/<entry_id>.json` with a
      non-empty transcript (or the documented `[BLANK_AUDIO]`
      silence hallucination for the synthesized sine-wave
      fixture).

## Sign-off

- [ ] All boxes ticked above.
- [ ] No console errors during any of the above flows.
- [ ] Pedro's signature + date here: ________________________

## Troubleshooting quick-reference

| Symptom | Likely cause | Fix |
|---|---|---|
| TCC dialog never appears | Already granted/denied previously | Check System Settings → Privacy & Security → Microphone |
| `MODEL NOT FOUND` printed by the e2e script | Whisper model not downloaded | Run the §5.1 `model_manager::ensure_model` (or the in-app "Download model" button) |
| Hotkey does nothing | Another app owns the shortcut | Use the tray menu's "Change hotkey" item to pick a different combo |
| Tray icon does not blink | Mic permission denied | Open tray menu → "Open Mic Settings" |
| Transcript is empty | Audio was silence (no speech in fixture) | This is expected for the synthesized 5-sec sine wave; speak during the live capture to see a populated transcript |
| Popover shows error toast on stop | Stream was already aborted by the user | This is the abort handler's documented path; no action needed |
