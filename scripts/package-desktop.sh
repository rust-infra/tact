#!/usr/bin/env bash
# Packages the TSX desktop with `nsh package` and stages the archive the
# release workflows publish.
#
#   package-desktop.sh <macos|linux|windows> <version> <archive>
#
# Environment: CLI_BIN (the built `tact-ui` binary), APP_DIR (default
# `crates/desktop`), SOURCE_DATE_EPOCH. On macOS, MACOS_SIGNING_IDENTITY and
# the notarization secrets sign and notarize the bundle when all are set.
set -euo pipefail

os="${1:?target os: macos, linux or windows}"
version="${2:?version}"
asset="${3:?output archive}"
app_dir="${APP_DIR:-crates/desktop}"
cli_bin="${CLI_BIN:?CLI_BIN names the built tact-ui binary}"
repo="$(cd "$(dirname "$0")/.." && pwd)"
source_date="${SOURCE_DATE_EPOCH:-$(git -C "${repo}" log -1 --format=%ct)}"
name="Tact Desktop"

work="$(mktemp -d)"
trap 'rm -rf "${work}"' EXIT

(cd "${app_dir}" && nsh package --target "${os}" --app-version "${version}" -o "${work}/pkg")

stage="${work}/stage"
mkdir -p "${stage}"
cp "${repo}/LICENSE" "${stage}/LICENSE"
cp "${repo}/LICENSE" "${stage}/LICENSE.tact-ui"
cp "${repo}/THIRD-PARTY-NOTICES.md" "${repo}/THIRD-PARTY-APACHE-2.0.txt" \
  "${repo}/Cargo.lock" "${stage}/"
if [[ -f "${repo}/THIRD-PARTY-LICENSES.txt" ]]; then
  cp "${repo}/THIRD-PARTY-LICENSES.txt" "${stage}/"
fi
cp "${repo}/crates/desktop/assets/fonts/IBM-Plex-LICENSE.txt" \
  "${repo}/crates/desktop/assets/fonts/Lilex-LICENSE.txt" "${stage}/"

case "${os}" in
  macos)
    app="${stage}/${name}.app"
    cp -R "${work}/pkg/${name}.app" "${app}"
    mv "${app}/Contents/MacOS/TactDesktop" "${app}/Contents/MacOS/tact-desktop"
    cp "${cli_bin}" "${app}/Contents/MacOS/tact-ui"
    cp "${stage}"/LICENSE* "${stage}"/*LICENSE*.txt "${stage}"/THIRD-PARTY-* \
      "${stage}/Cargo.lock" "${app}/Contents/Resources/"
    /usr/libexec/PlistBuddy -c "Set :CFBundleExecutable tact-desktop" "${app}/Contents/Info.plist"
    if [[ -n "${MACOS_SIGNING_IDENTITY:-}" ]]; then
      : "${MACOS_CERTIFICATE:?}"
      : "${MACOS_CERTIFICATE_PASSWORD:?}"
      : "${APPLE_ID:?}"
      : "${APPLE_TEAM_ID:?}"
      : "${APPLE_APP_PASSWORD:?}"
      keychain="${RUNNER_TEMP:-/tmp}/tact-signing.keychain-db"
      cert="${RUNNER_TEMP:-/tmp}/tact-signing.p12"
      printf '%s' "${MACOS_CERTIFICATE}" | base64 --decode > "${cert}"
      security create-keychain -p tact-signing "${keychain}"
      security set-keychain-settings -lut 21600 "${keychain}"
      security unlock-keychain -p tact-signing "${keychain}"
      security import "${cert}" -k "${keychain}" -P "${MACOS_CERTIFICATE_PASSWORD}" \
        -T /usr/bin/codesign
      security set-key-partition-list -S apple-tool:,apple: \
        -s -k tact-signing "${keychain}" >/dev/null
      security list-keychains -d user -s "${keychain}" \
        $(security list-keychains -d user | tr -d '"')
      codesign --force --deep --options runtime --timestamp \
        --sign "${MACOS_SIGNING_IDENTITY}" "${app}"
      codesign --verify --deep --strict --verbose=2 "${app}"
      notarize_zip="${work}/notarize.zip"
      ditto -c -k --sequesterRsrc --keepParent "${app}" "${notarize_zip}"
      xcrun notarytool submit "${notarize_zip}" \
        --apple-id "${APPLE_ID}" --team-id "${APPLE_TEAM_ID}" \
        --password "${APPLE_APP_PASSWORD}" --wait
      xcrun stapler staple "${app}"
      xcrun stapler validate "${app}"
      spctl --assess --type execute --verbose=4 "${app}"
    else
      codesign --force --deep --sign - "${app}"
    fi
    python3 "${repo}/scripts/normalize-mtime.py" "${app}"
    (cd "${stage}" && python3 "${repo}/scripts/package-zip.py" \
      --output "${asset}" "${name}.app")
    ;;
  windows)
    cp -R "${work}/pkg/${name}/nsh" "${stage}/nsh"
    cp "${work}/pkg/${name}/${name}.exe" "${stage}/tact-desktop.exe"
    cp "${cli_bin}" "${stage}/tact-ui.exe"
    python3 "${repo}/scripts/normalize-mtime.py" "${stage}"/*
    (cd "${stage}" && python3 "${repo}/scripts/package-zip.py" --output "${asset}" \
      tact-desktop.exe tact-ui.exe LICENSE LICENSE.tact-ui \
      IBM-Plex-LICENSE.txt Lilex-LICENSE.txt THIRD-PARTY-NOTICES.md \
      THIRD-PARTY-APACHE-2.0.txt THIRD-PARTY-LICENSES.txt Cargo.lock nsh)
    ;;
  linux)
    cp -R "${work}/pkg/${name}/nsh" "${stage}/nsh"
    cp "${work}/pkg/${name}/TactDesktop" "${stage}/tact-desktop"
    cp "${cli_bin}" "${stage}/tact-ui"
    strip "${stage}/tact-desktop" || true
    strip "${stage}/tact-ui" || true
    python3 "${repo}/scripts/normalize-mtime.py" "${stage}"/*
    tar --sort=name --mtime="@${source_date}" --owner=0 --group=0 \
      --numeric-owner -czf "${asset}" -C "${stage}" .
    ;;
  *)
    echo "unknown target: ${os}" >&2
    exit 2
    ;;
esac

echo "PACKAGE_PATH=${asset}"
