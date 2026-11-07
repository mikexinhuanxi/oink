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
