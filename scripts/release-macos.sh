#!/usr/bin/env bash
set -euo pipefail

APP_PATH="src-tauri/target/release/bundle/macos/Peer.app"
DMG_DIR="src-tauri/target/release/bundle/dmg"
VERSION="$(node -p "require('./package.json').version")"
ARCH="$(uname -m)"
DMG_VERSIONED_PATH="${DMG_DIR}/Peer_${VERSION}_${ARCH}.dmg"
DMG_LATEST_PATH="${DMG_DIR}/Peer.dmg"
IDENTITY="${APPLE_SIGNING_IDENTITY:-}"
TEAM_ID="${APPLE_TEAM_ID:-}"
NOTARY_PROFILE="${APPLE_NOTARYTOOL_PROFILE:-}"
ALLOW_UNSIGNED="${PEER_ALLOW_UNSIGNED_RELEASE:-0}"

if [[ "${ALLOW_UNSIGNED}" != "1" ]]; then
  if [[ -z "${IDENTITY}" ]]; then
    echo "APPLE_SIGNING_IDENTITY is required for a public macOS release." >&2
    echo "Use a Developer ID Application identity, or set PEER_ALLOW_UNSIGNED_RELEASE=1 for a local smoke build." >&2
    exit 1
  fi

  if [[ "${IDENTITY}" != Developer\ ID\ Application:* ]]; then
    echo "APPLE_SIGNING_IDENTITY must be a Developer ID Application identity for public distribution." >&2
    echo "Current value: ${IDENTITY}" >&2
    exit 1
  fi

  if [[ -z "${NOTARY_PROFILE}" ]]; then
    echo "APPLE_NOTARYTOOL_PROFILE is required for a public macOS release." >&2
    echo "Create one with: xcrun notarytool store-credentials <profile-name>" >&2
    exit 1
  fi
elif [[ -z "${IDENTITY}" ]]; then
  echo "PEER_ALLOW_UNSIGNED_RELEASE=1: building a local ad-hoc signed DMG that Gatekeeper will reject." >&2
fi

strip_launchservices_blockers() {
  local plist="${APP_PATH}/Contents/Info.plist"
  if /usr/libexec/PlistBuddy -c "Print :LSRequiresCarbon" "${plist}" >/dev/null 2>&1; then
    /usr/libexec/PlistBuddy -c "Delete :LSRequiresCarbon" "${plist}"
  fi
}

pnpm sidecar
pnpm tauri build --bundles app

strip_launchservices_blockers

if [[ -n "${IDENTITY}" ]]; then
  codesign --force --deep --options runtime \
    --timestamp \
    --sign "${IDENTITY}" \
    --entitlements src-tauri/entitlements.plist \
    "${APP_PATH}"
else
  codesign --force --deep --sign - "${APP_PATH}"
fi

codesign --verify --deep --strict --verbose=2 "${APP_PATH}"

mkdir -p "${DMG_DIR}"
rm -f "${DMG_VERSIONED_PATH}" "${DMG_LATEST_PATH}"
hdiutil create -volname "Peer" \
  -srcfolder "${APP_PATH}" \
  -ov \
  -format UDZO \
  "${DMG_VERSIONED_PATH}"

if [[ -n "${IDENTITY}" ]]; then
  codesign --force --timestamp --sign "${IDENTITY}" "${DMG_VERSIONED_PATH}"
fi

if [[ -n "${IDENTITY}" && -n "${NOTARY_PROFILE}" ]]; then
  notary_args=(--keychain-profile "${NOTARY_PROFILE}")
  if [[ -n "${TEAM_ID}" ]]; then
    notary_args+=(--team-id "${TEAM_ID}")
  fi
  xcrun notarytool submit "${DMG_VERSIONED_PATH}" \
    "${notary_args[@]}" \
    --wait
  xcrun stapler staple "${DMG_VERSIONED_PATH}"
fi

cp "${DMG_VERSIONED_PATH}" "${DMG_LATEST_PATH}"

echo "Built DMG at ${DMG_VERSIONED_PATH}"
echo "Website upload asset at ${DMG_LATEST_PATH}"
