#!/bin/sh
set -eu

bundle_id="com.unsoldgroup.commonkit"

fail() {
  printf 'FAIL: %s\n' "$*" >&2
  exit 1
}

activation_policy() {
  BUNDLE_ID="$bundle_id" swift -e '
    import AppKit
    import Foundation

    let bundleID = ProcessInfo.processInfo.environment["BUNDLE_ID"]!
    let apps = NSRunningApplication.runningApplications(withBundleIdentifier: bundleID)
    guard let app = apps.first else {
      FileHandle.standardError.write(Data("CommonKit is not running\n".utf8))
      exit(69)
    }
    print(app.activationPolicy.rawValue)
  '
}

window_count() {
  osascript -e 'tell application "System Events" to tell process "CommonKit" to get count of windows'
}

dock_contains_commonkit() {
  osascript -e 'tell application "System Events" to tell process "Dock" to get name of every UI element of list 1' |
    tr ',' '\n' |
    sed 's/^ *//;s/ *$//' |
    grep -Fxq CommonKit
}

wait_for_state() {
  expected_policy="$1"
  expected_windows="$2"
  expected_dock="$3"
  description="$4"
  attempts=0

  while [ "$attempts" -lt 40 ]; do
    policy="$(activation_policy 2>/dev/null || true)"
    windows="$(window_count 2>/dev/null || true)"
    if dock_contains_commonkit 2>/dev/null; then
      dock="present"
    else
      dock="absent"
    fi
    if [ "$policy" = "$expected_policy" ] &&
      [ "$windows" = "$expected_windows" ] &&
      [ "$dock" = "$expected_dock" ]; then
      printf 'PASS: %s\n' "$description"
      return 0
    fi
    attempts=$((attempts + 1))
    sleep 0.25
  done

  fail "$description (policy=${policy:-unavailable}, windows=${windows:-unavailable}, Dock=$dock)"
}

[ "$(uname -s)" = "Darwin" ] || fail "this smoke test requires macOS"
if activation_policy >/dev/null 2>&1; then
  fail "CommonKit is already running; quit it from the Ck menu and rerun this smoke test"
fi

printf '%s\n' "Opening CommonKit. macOS may ask for Accessibility permission so this script can inspect the Dock and window state."
open -a CommonKit
wait_for_state 0 1 present "normal launch/open expected Regular with one window and a Dock icon"

printf '%s' "Close CommonKit with the red window button, then press Return: "
read -r _
wait_for_state 1 0 absent "close expected Accessory with no window or Dock icon while the process remains"

printf '%s' 'Choose “Open CommonKit” from the Ck menu-bar menu, then press Return: '
read -r _
wait_for_state 0 1 present "Open CommonKit expected Regular with one window and a Dock icon"

printf '%s\n' "CommonKit macOS Dock lifecycle passed."
