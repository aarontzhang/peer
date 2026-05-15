# TODO — Ship Peer beta to a first external tester

Owner: Aaron. Tasks here require credentials, a browser, or human judgement Claude can't do.
The corresponding Claude-side work is tracked in `/Users/aaronzhang/.claude/plans/ok-i-want-you-whimsical-manatee.md`.

Production URL: **https://www.peercv.com**
GitHub repo: **https://github.com/aarontzhang/peer** (already public)

---

## ✅ Already verified by Claude (no action needed)

These are done — listed so you know the current state.

- [x] **Vercel project `peer` is live** at `www.peercv.com`. `/` returns 200, `/download/latest` returns a clean 302 to GitHub Releases.
- [x] **All required env vars** are set on Vercel production (OPENAI_API_KEY, ANTHROPIC_API_KEY, SUPABASE_URL, SUPABASE_SERVICE_ROLE_KEY, SUPABASE_ANON_KEY, SUPABASE_JWT_SECRET, PEER_BACKEND_URL). `PEER_MACOS_DOWNLOAD_URL` is not set — that's fine; `api/download-latest.js` has the correct default baked in.
- [x] **Sign-in is open beta, no invite code.** The auth flow is Google OAuth via Supabase implicit flow (`src-tauri/src/saas.rs:187`). There is no invite-code prompt in the desktop app and no code that reads `PEER_BETA_INVITE_CODE`.
- [x] **The free beta recording cap is enforced.** `PEER_FREE_BETA_MONTHLY_LIMIT` defaults to 25 completed recordings per Supabase user per UTC calendar month. Empty or invalid values fall back to 25, and over-limit recording API calls return `429` with `{ "error": "monthly beta recording limit reached" }`.
- [x] **Supabase auth callbacks** allow the current loopback callback (`http://127.0.0.1:17643/auth-callback`) and keep `/api/auth-callback` for old builds that still use `peer://auth`.
- [x] **The GitHub release artifact exists.** `https://www.peercv.com/download/latest` resolves to `Peer.dmg`. Do not replace it with another self-signed or unsigned build; public replacement DMGs must be Developer ID signed and notarized.

---

## 1. Publish the v0.2.0 GitHub release  ⏱ ~3 minutes

After `pnpm release:mac` completes with Developer ID signing and notarization, upload `src-tauri/target/release/bundle/dmg/Peer.dmg` as the release asset so `/download/latest` resolves to the fixed build.

### Easiest path — let Claude run `gh release create`
Just say: **"go ahead and publish the release."**
Claude will run:
```sh
gh release create v0.2.0 \
  src-tauri/target/release/bundle/dmg/Peer.dmg \
  --title "Peer 0.2.0 (beta)" \
  --notes-file <( echo "First public beta. macOS Apple Silicon only.\n\nFirst-time install: open the DMG, drag Peer to Applications, then open Peer from Applications. Click Sign in to create an account with Google." ) \
  --prerelease
```

### Manual path (if you prefer the UI)
1. Open https://github.com/aarontzhang/peer/releases/new
2. **Choose a tag** → type `v0.2.0` → "Create new tag: v0.2.0 on publish"
3. **Title:** `Peer 0.2.0 (beta)`
4. **Description:**
   ```
   First public beta. macOS Apple Silicon only.

   First-time install: open the DMG, drag Peer to Applications,
   then open Peer from Applications. Click Sign in to create an account with Google.
   ```
5. **Attach the DMG:** drag the file from `src-tauri/target/release/bundle/dmg/Peer.dmg` into the assets box.
   ⚠️ The asset **must be named exactly `Peer.dmg`** — without the version suffix — because `api/download-latest.js` redirects to that exact filename. `pnpm release:mac` now writes this file directly.
6. Tick **"Set as a pre-release"**.
7. Click **Publish release**.

### How to confirm it worked
After publishing, in a terminal:
```sh
curl -sIL https://www.peercv.com/download/latest | head -25
curl -L https://www.peercv.com/download/latest -o Peer.dmg
codesign -dv --verbose=4 Peer.dmg
spctl -a -t open --context context:primary-signature -vv Peer.dmg
hdiutil attach Peer.dmg
defaults read /Volumes/Peer/Peer.app/Contents/Info CFBundleIdentifier
hdiutil detach /Volumes/Peer
```
You want to see redirects ending in `200` with `content-disposition: attachment; filename=Peer.dmg`. `spctl` must accept the DMG, and the mounted app bundle identifier must be `com.aaronzhang.peer`. If the last hop is 404, the asset name on the release page isn't `Peer.dmg` — rename it in the release UI.

---

## 2. Confirm Supabase signups are open + tidy beta env vars  ⏱ ~3 minutes

The desktop app's sign-in is just **Google OAuth via Supabase** — no invite code, no email allowlist in code. Whether new people can sign up depends entirely on one Supabase toggle.

### Confirm signups are open in Supabase
1. Open https://supabase.com/dashboard → select the Peer project (the one whose URL is in `SUPABASE_URL` on Vercel — `hmkpgxlfxwztficbuktj.supabase.co` per the hardcoded fallback in `src-tauri/src/saas.rs:20`).
2. Go to **Authentication → Sign In / Providers**.
3. Confirm **Google** is enabled and the OAuth client ID / secret are set. (They must be — your dev sign-in works.)
4. Go to **Authentication → Sign In / Up → User Signups**.
5. Make sure **"Allow new users to sign up"** is **ON**. If it's off, only existing users can sign in and new visitors will hit a wall.
6. Make sure the **Site URL** (Authentication → URL Configuration) and **Redirect URLs** include `http://127.0.0.1:17643/auth-callback` for current desktop builds. Keep `https://www.peercv.com/api/auth-callback` and `https://peer-wheat.vercel.app/api/auth-callback` whitelisted for old builds.

### Remove the stale invite env var from Vercel
`PEER_BETA_INVITE_CODE` is not consumed by the desktop app or backend. Safe to delete:
1. Go to https://vercel.com/aaronzhangper-gmailcoms-projects/peer/settings/environment-variables
2. Delete `PEER_BETA_INVITE_CODE`.
3. Keep `PEER_FREE_BETA_MONTHLY_LIMIT` if you want a non-default cap. Leaving it empty or setting an invalid value falls back to 25.
4. Redeploy after changing env vars so the active Vercel functions pick them up.

### Share with the tester
Send your tester:
- The link: **https://www.peercv.com**
- A short note: *"Click Download. After it downloads, open the DMG, drag Peer into Applications, then open Peer. When Peer launches, click Sign in — it'll open a Google sign-in tab in your browser. Use any Google account."*

---

## 3. Smoke-test it yourself before sharing  ⏱ ~10 minutes  (HIGHLY RECOMMENDED)

Don't send the link to an external tester until you've done one full clean-room install. Easiest way: do this on a Mac that isn't your dev machine, **or** delete your local `Peer.app` from `/Applications` first and pretend you're a new user.

### Steps
1. Open **https://www.peercv.com** in a fresh browser tab.
2. Click **Download for macOS** → DMG downloads.
3. Open the DMG in Finder, drag `Peer.app` into Applications.
4. Open Applications → `Peer.app`. A notarized build should open without the right-click Gatekeeper workaround.
5. macOS will prompt for three permissions in System Settings → Privacy & Security. Grant all three:
   - **Screen & System Audio Recording**
   - **Microphone**
   - **Accessibility** (required for the Right Option / Fn key-tap detection)
6. When the pill window appears, click the sign-in button. A browser tab opens to Google sign-in (Supabase OAuth implicit flow) → pick a Google account → after sign-in the browser returns to `http://127.0.0.1:17643/auth-callback` and Peer stores the token in macOS Keychain.
7. Tap **Right Option** to start recording → narrate a 5-second screen task → tap **Right Option** again to stop.
8. The result window should populate with markdown within ~12 seconds.

### If sign-in fails
- Most likely cause: the redirect URL `http://127.0.0.1:17643/auth-callback` isn't whitelisted in Supabase. Fix in Supabase → Authentication → URL Configuration (see Task 2 above).
- Second most likely: Google OAuth client in Supabase doesn't have the Supabase auth callback URL `https://<project>.supabase.co/auth/v1/callback` in its Authorized redirect URIs. Fix in the Google Cloud Console for the OAuth client tied to Supabase.
- If you see a Keychain prompt asking to allow Peer to access an item, click **Always Allow**.

### If a recording fails
- Check `~/Library/Logs/Peer/` for Rust logs.
- Common cause: Screen Recording permission was granted to an old build's code signature. Run the TCC reset block from `README.md` ("TCC reset" section).

---

## 4. (Optional but valuable later) Things to consider for the next iteration

Not blocking the current ship, but worth tracking:

- **Intel Mac users will be locked out.** The current DMG is `arm64` only. ~50% of macOS active-user base is still Intel as of late 2025. If you want broader testing, ask Claude to build a universal binary (`pnpm tauri build --target universal-apple-darwin`). Adds ~30min to build time and ~2x to DMG size.
- **Gatekeeper blocks self-signed builds.** The release script now refuses public builds unless `APPLE_SIGNING_IDENTITY` is a Developer ID Application identity and `APPLE_NOTARYTOOL_PROFILE` is set. This machine currently only has `Peer Self Signed`, so Aaron still needs Apple Developer Program credentials before publishing a replacement DMG.
- **No GitHub Actions release workflow.** Every release is currently a manual `pnpm release:mac` on this machine. When you have notarization set up, ask Claude to add `.github/workflows/release.yml` using `tauri-apps/tauri-action` so tag pushes auto-build and release.
- **No in-app updater.** Testers who installed beta-1 will not be auto-prompted for beta-2. For 1–5 testers this is fine; for >10 it's a pain. Tauri's updater plugin is the path.
- **No way to revoke a tester's access.** Sign-in is open via Google OAuth. If you want per-tester revocation, add a small allowlist/admin endpoint keyed by Supabase user.

---

## Status check — is the DMG done building?

To check a release build:
```sh
ls -la src-tauri/target/release/bundle/dmg/
# Look for Peer.dmg and Peer_0.2.0_arm64.dmg with today's timestamp.
```
If the public build exits before compiling, the missing Developer ID / notary credentials are still not configured. For local-only launch smoke testing, use `PEER_ALLOW_UNSIGNED_RELEASE=1 pnpm release:mac`; never upload that unsigned build.
