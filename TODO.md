# Peer Feature Roadmap

## Processing Speed (active)

- [x] Keep the current pipeline shape: ffprobe -> keyframes + transcription in parallel -> per-window visual notes in parallel -> final text aggregation.
- [x] Use Claude Sonnet 4.6 for per-window visual analysis.
- [x] Preserve current OpenAI Whisper transcription for the first pass.
- [x] Add stage timing logs for probe, keyframe extraction, audio extraction/transcription, Claude vision windows, Claude aggregation, and total time.
- [x] Add failure isolation: failed Claude vision windows are logged and skipped; if all visual windows fail but transcript exists, Claude still runs with transcript-only context.
- [x] Keep current UI behavior: transcript and thinking stream as they do now; final prompt still auto-copies.
- [ ] Benchmark a typical short recording and confirm post-recording latency visibly improves, with a stretch target near 5 seconds.
- [ ] If benchmarking shows transcription is the bottleneck, evaluate newer transcription models as a follow-up.

## Configurable Recording Keybind (pending)

- [x] Change the default recording keybind to Fn tap.
- [x] Keep Fn tap and Cmd+Shift+R as supported fallback options.
- [x] Add a settings UI control for recording keybind configuration.
- [x] Implement press-to-capture-shortcut behavior.
- [x] Persist keybind configuration locally and re-register global shortcuts on launch and after changes.
- [x] Show a clear unavailable-state message if a chosen key requires Accessibility permission or cannot be registered.
- [x] Keep pill click-to-record.

## Ship-Ready SaaS Release (pending)

- [ ] Target macOS only for the first downloadable release.
- [ ] Move from local provider keys to a managed SaaS backend using Vercel and Supabase.
- [ ] Use a free beta release model with usage limits, not paid subscriptions on day one.
- [ ] Add desktop authentication through browser login returning a device token or deep link, stored in macOS Keychain.
- [ ] Build a download website with account login, latest macOS download, onboarding notes for Screen Recording/Microphone/Accessibility permissions, and basic release/version info.
- [ ] Replace ad-hoc local signing with a proper macOS release path: production bundle identifier, Developer ID signing, notarization, DMG target, and release automation.

## Public Interfaces And Types

- [x] Keep the visual analysis provider boundary so window observations can change provider without changing downstream aggregation shape.
- [x] Keep the existing per-window observation JSON contract: `userSpeech`, `pointing`, and `visibleContext`.
- [x] Add timing telemetry as internal structured logs first; no user-facing schema change required for Milestone 1.
- [ ] Later milestones add backend auth/API interfaces.

## Test Plan

- [x] Run `pnpm build`.
- [x] Run `cargo check`.
- [x] Run `cargo fmt --check`.
- [x] Run `cargo test`.
- [x] Run Swift sidecar build.
- [ ] Run a short manual recording and confirm final prompt quality remains usable.
- [ ] Confirm logs show per-stage timing and total processing time.
- [ ] Test degraded cases: no frames, failed visual window, transcript-only recording.
