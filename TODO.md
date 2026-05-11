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
- [x] **Sign-in is open beta, no invite code.** The auth flow is Google OAuth via Supabase implicit flow (`src-tauri/src/saas.rs:187`). There is no invite-code prompt in the desktop app and no code that reads `PEER_BETA_INVITE_CODE` — that Vercel env var is dormant. Same for `PEER_FREE_BETA_MONTHLY_LIMIT` (the `recording_usage` table only logs; no code enforces a cap). Both env vars can be deleted from Vercel; they're cosmetic. (See Task 2 below.)
- [x] **Supabase auth callback** (`/api/auth-callback`) returns 200 and contains the `peer://auth` deep-link logic.
- [x] **The single missing piece** is the GitHub release artifact — `https://github.com/aarontzhang/peer/releases/latest/download/Peer.dmg` returns 404 today because no release exists yet. Tasks 1–3 below fix that.

---

## 1. Publish the v0.2.0 GitHub release  ⏱ ~3 minutes

After Claude finishes rebuilding the DMG (see "Status check" below), you need to publish it as a GitHub release so `/download/latest` resolves.

### Easiest path — let Claude run `gh release create`
Just say: **"go ahead and publish the release."**
Claude will run:
```sh
gh release create v0.2.0 \
  src-tauri/target/release/bundle/dmg/Peer.dmg \
  --title "Peer 0.2.0 (beta)" \
  --notes-file <( echo "First public beta. macOS Apple Silicon only.\n\nFirst-time install: open the DMG, drag Peer to Applications, right-click Peer → Open. macOS will ask once; click Open again." ) \
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
   right-click Peer → Open. macOS will ask once; click Open again.
   ```
5. **Attach the DMG:** drag the file from `src-tauri/target/release/bundle/dmg/Peer.dmg` into the assets box.
   ⚠️ The asset **must be named exactly `Peer.dmg`** — without the version suffix — because `api/download-latest.js` redirects to that exact filename. Claude will rename it before you upload.
6. Tick **"Set as a pre-release"**.
7. Click **Publish release**.

### How to confirm it worked
After publishing, in a terminal:
```sh
curl -sIL https://www.peercv.com/download/latest | head -25
```
You want to see `HTTP/2 302 → HTTP/2 302 → HTTP/2 200` with `content-type: application/octet-stream` and a `content-length` of roughly 4–8 MB. If the last hop is 404, the asset name on the release page isn't `Peer.dmg` — rename it in the release UI.

---

## 2. Confirm Supabase signups are open + tidy unused Vercel env vars  ⏱ ~3 minutes

The desktop app's sign-in is just **Google OAuth via Supabase** — no invite code, no email allowlist in code. Whether new people can sign up depends entirely on one Supabase toggle.

### Confirm signups are open in Supabase
1. Open https://supabase.com/dashboard → select the Peer project (the one whose URL is in `SUPABASE_URL` on Vercel — `hmkpgxlfxwztficbuktj.supabase.co` per the hardcoded fallback in `src-tauri/src/saas.rs:20`).
2. Go to **Authentication → Sign In / Providers**.
3. Confirm **Google** is enabled and the OAuth client ID / secret are set. (They must be — your dev sign-in works.)
4. Go to **Authentication → Sign In / Up → User Signups**.
5. Make sure **"Allow new users to sign up"** is **ON**. If it's off, only existing users can sign in and new visitors will hit a wall.
6. Make sure the **Site URL** (Authentication → URL Configuration) and **Redirect URLs** include `https://www.peercv.com/api/auth-callback`. If only the `peer-wheat.vercel.app` URL is whitelisted, sign-in from the custom domain will fail with a redirect-not-allowed error.

### Remove the two unused env vars from Vercel (optional cosmetic cleanup)
Both `PEER_BETA_INVITE_CODE` and `PEER_FREE_BETA_MONTHLY_LIMIT` are not consumed by any deployed code. Safe to delete:
1. Go to https://vercel.com/aaronzhangper-gmailcoms-projects/peer/settings/environment-variables
2. Delete `PEER_BETA_INVITE_CODE` and `PEER_FREE_BETA_MONTHLY_LIMIT`.
3. No redeploy needed (the absence won't change behavior; the deployed code doesn't read them).

### Share with the tester
Send your tester:
- The link: **https://www.peercv.com**
- A short note: *"Click Download. After it downloads, open the DMG, drag Peer into Applications, then right-click Peer → Open (macOS will ask once because the beta isn't notarized — it's safe). When Peer launches, click Sign in — it'll open a Google sign-in tab in your browser. Use any Google account."*

---

## 3. Smoke-test it yourself before sharing  ⏱ ~10 minutes  (HIGHLY RECOMMENDED)

Don't send the link to an external tester until you've done one full clean-room install. Easiest way: do this on a Mac that isn't your dev machine, **or** delete your local `Peer.app` from `/Applications` first and pretend you're a new user.

### Steps
1. Open **https://www.peercv.com** in a fresh browser tab.
2. Click **Download for macOS** → DMG downloads.
3. Open the DMG in Finder, drag `Peer.app` into Applications.
4. Open Applications → **right-click `Peer.app` → Open** → confirm at the Gatekeeper dialog.
5. macOS will prompt for three permissions in System Settings → Privacy & Security. Grant all three:
   - **Screen & System Audio Recording**
   - **Microphone**
   - **Accessibility** (required for the Fn key-tap detection)
6. When the pill window appears, click the sign-in button. A browser tab opens to Google sign-in (Supabase OAuth implicit flow) → pick a Google account → after sign-in you should be deep-linked back to `peer://auth`. Token gets stored in macOS Keychain.
7. Press **Fn** to start recording → narrate a 5-second screen task → press **Fn** again to stop.
8. The result window should populate with markdown within ~12 seconds.

### If sign-in fails
- Most likely cause: the redirect URL `https://www.peercv.com/api/auth-callback` isn't whitelisted in Supabase. Fix in Supabase → Authentication → URL Configuration (see Task 2 above).
- Second most likely: Google OAuth client in Supabase doesn't have `https://www.peercv.com/api/auth-callback` (or the Supabase auth callback URL `https://<project>.supabase.co/auth/v1/callback`) in its Authorized redirect URIs. Fix in the Google Cloud Console for the OAuth client tied to Supabase.
- If you see a Keychain prompt asking to allow Peer to access an item, click **Always Allow**.

### If a recording fails
- Check `~/Library/Logs/Peer/` for Rust logs.
- Common cause: Screen Recording permission was granted to an old build's code signature. Run the TCC reset block from `README.md` ("TCC reset" section).

---

## 4. (Optional but valuable later) Things to consider for the next iteration

Not blocking the current ship, but worth tracking:

- **Intel Mac users will be locked out.** The current DMG is `arm64` only. ~50% of macOS active-user base is still Intel as of late 2025. If you want broader testing, ask Claude to build a universal binary (`pnpm tauri build --target universal-apple-darwin`). Adds ~30min to build time and ~2x to DMG size.
- **Gatekeeper friction is real.** Even with the install note on the landing page, a meaningful fraction of testers will bounce. The fix is a $99/yr Apple Developer Program → Developer ID Application cert → notarytool credentials. The release script already handles signing + notarization when those env vars are present; you'd just need to add `APPLE_SIGNING_IDENTITY` and `APPLE_NOTARYTOOL_PROFILE` to your shell and rerun `pnpm release:mac`.
- **No GitHub Actions release workflow.** Every release is currently a manual `pnpm release:mac` on this machine. When you have notarization set up, ask Claude to add `.github/workflows/release.yml` using `tauri-apps/tauri-action` so tag pushes auto-build and release.
- **No in-app updater.** Testers who installed beta-1 will not be auto-prompted for beta-2. For 1–5 testers this is fine; for >10 it's a pain. Tauri's updater plugin is the path.
- **No way to revoke a tester's access.** The beta gate is one shared invite code. If you want per-tester revocation, you'd want a small admin endpoint that issues per-user codes against Supabase.

---

## Status check — is the DMG done building?

Claude kicked off `pnpm release:mac` in the background. To check progress:
```sh
ls -la src-tauri/target/release/bundle/dmg/
# Look for Peer_0.2.0_arm64.dmg with today's timestamp.
```
If the timestamp is fresh (today), the build is done and Claude will rename it to `Peer.dmg` before publishing. If you see the May-10 file still, the build is still running or failed — paste the tail of the output to Claude.
