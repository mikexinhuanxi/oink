# macOS test-build window controls

The Mos compatibility change leaves the frontend layout and official traffic-light
inset unchanged. On this macOS 27.0.1 host, the temporary test build could initially
position the controls correctly and later have AppKit reset them to its default
outer titlebar position, in the 24-point transparent shadow gutter. Native
measurements observed the close-button rectangle change from `(42, 39, 14, 14)`
to `(9, 9, 14, 14)` in WebView points.

The local window fix restores the existing `traffic_light_position(42, 48)` policy
for the main window after native preparation, reveal/focus, and titlebar alignment
measurements. It uses the same titlebar-container geometry as the pinned Wry
implementation, preserves native button spacing, skips fullscreen, and does not
change CSS, the frontend layout, the gutter, or Mos input handling.

Validation on 2026-11-07: the input-disabled, isolated build-102 backend started and
its packaged CLI passed IPC. The native macOS window geometry tests passed (3),
and the existing frontend titlebar/alignment tests passed (7). The reporter viewed
the isolated build-102 main window and confirmed the three buttons were inside the
window again. Fullscreen/minimize/resize coverage on other macOS versions remains
unverified. The already-authorized build-101 installation was not overwritten.

This test-package window fix is kept on a separate branch from the Mos contribution.

## Build 102 recurrence and native update regression

The reporter subsequently observed the controls outside the shell again in the
installed build 102. Its initial acceptance did not cover all later native layout
updates. Restoring during reveal and WebView alignment queries was insufficient.

The main window now also observes its own `NSWindowDidUpdateNotification` on the
AppKit thread. After the native update it reapplies the same official inset when
necessary. It performs no polling, ignores fullscreen through the existing guard,
and uses an idempotent frame update plus a synchronous reentry guard. Closing the
main window removes the observer and releases its retained window.

`cargo test -p oink-ui --test macos_window_controls` runs real AppKit on the main
thread without showing windows or posting input. It explicitly reproduces the
measured default layout, verifies that a one-time restoration leaves it outside,
and then verifies recovery over 20 reset/update cycles, repeated-update stability,
window isolation, and observer removal. Hidden AppKit windows do not reset on every
style/title change, so the test deliberately sets the measured stale frames before
calling the real native update; it does not claim those operations always trigger
the reset on every macOS version.

Build-103 acceptance: the packaged backend/CLI passed isolated IPC. The actual
isolated WebView went through native fullscreen exit and a minimize/raise sequence;
the reporter confirmed the controls remained inside the window afterwards. The
fixed user-Applications copy was then updated to build 103, with unchanged pairing
and frontend layout. After explicit consent, only its Accessibility and ListenEvent
records were reset and its device-control grant re-added. The installed UI showed
build 103, shared mouse/keyboard, and the existing Windows peer online.

When replacing a bundle at the same path, System Settings' open panel can retain
a file reference to the old bundle after it has moved to Trash, even while the
visible column still says Applications. Verify the selected file URL. Restarting
System Settings cleared this cached reference; the exact installed application URL
was then selected. Do not grant a cached Trash copy or modify/resign an app after
its permission registration. Temporary signing can require registration again on
future builds.
