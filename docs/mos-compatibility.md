# Mos scroll compatibility

The Mac input backend automatically adopts the direction policy of an active Mos
scroll filter. It reads Mos preferences and enumerates the public event-tap list
on the main queue every 500 ms. Installing Mos alone, quitting it, or disabling its
scroll tap does not enable compensation. No Mos preferences are changed.

## Capture and delivery

Oink captures at Session, before Mos's AnnotatedSession filter. Mos can consume
ordinary wheel events and send its smoothed frames directly to the application
PID, bypassing the capture chain. Moving Oink after Mos would therefore lose
scrolls. Instead, Oink applies Mos's horizontal and vertical reversal to the
**forwarded copy** of an unphased mouse wheel. Both accelerated line amounts and
raw wheel steps change together, covering both forwarding modes. The source-local
CGEvent is untouched, so Mos retains its ordinary local smoothing behavior.
Touchpads, continuous mouse scrolling classified as touchpad input, and phased
scrolls keep their existing paths. Remote wheel speed and delivery remain Oink's;
this does not reproduce or transmit Mos's smoothing curve to Windows.

The snapshot includes Mos's global direction, axis gates, allowlist, and per-app
direction overrides keyed by the target PID supplied by Quartz. Where Session
events do not yet identify a target application, the global/allowlist fallback
applies; no application identity is invented from the remote computer's focus.

For Windows → Mac, Oink first acknowledges its own injected scroll with its own
original marker at Session. It then replaces a copied event source with Mos's
processed-scroll marker (CoreGraphics caches source fields, so a direct integer
field write after reading the old marker is ineffective). The shared injector
source is unchanged. Mos passes that event through without consuming,
reversing or re-smoothing it. Deltas, phases, flags, position and timestamp remain
unchanged. Only this process's scroll events can use the compatibility identity;
real Mos frames from another process remain foreign input to Oink.

The existing “Reverse mouse wheel” preference still belongs to the computer the
mouse is connected to. It composes with source compensation. Changing the
Windows source preference does not affect a Mac-connected mouse.

## Compatibility contract

This adapter was checked against Mos **4.2.1**, commit
[`0ff06a4`](https://github.com/Caldis/Mos/tree/0ff06a4e787abd5d8568e31865481e75abe550b4):

- [ScrollCore.swift](https://github.com/Caldis/Mos/blob/0ff06a4e787abd5d8568e31865481e75abe550b4/Mos/ScrollCore/ScrollCore.swift): AnnotatedSession placement, synthetic-scroll bypass, reversal and smoothing consumption.
- [ScrollUtils.swift](https://github.com/Caldis/Mos/blob/0ff06a4e787abd5d8568e31865481e75abe550b4/Mos/ScrollCore/ScrollUtils.swift): processed-scroll marker `0x4D4F53534D4F4F54`.
- [Options.swift](https://github.com/Caldis/Mos/blob/0ff06a4e787abd5d8568e31865481e75abe550b4/Mos/Options/Options.swift) and [Application.swift](https://github.com/Caldis/Mos/blob/0ff06a4e787abd5d8568e31865481e75abe550b4/Mos/Windows/PreferencesWindow/ApplicationView/Application.swift): preference keys, app rules and inheritance.

The processed-scroll marker is a Mos implementation convention, not a public
interoperability API. Recheck these sources and the live filter when upgrading
Mos. Native regression tests cover axis compensation, local/continuous exclusions,
app-rule inheritance, active-tap admission, and marker ownership/payload integrity.
Full acceptance also requires a modified Mac build paired with Windows, with Mos
running, stopped, and its direction settings changed while Oink stays open.

## Local hardware acceptance, 2026-11-07

The reporter confirmed both Mac-connected mouse → Windows and Windows-connected
mouse → Mac scrolling work with Mos running, using Mos 4.2.1 and the Mac test build
`0.3.3-inhouse.101` on macOS 27.0.1. Local Mac smoothing remains Mos-owned.

The test copy needed a fresh permission registration. Reusing the official bundle
identifier from a mounted disk image left an enabled Settings row while the test
backend still reported `NotGranted`. A fixed user-Applications copy with the
independent identifier `com.mikexinhuanxi.oink.mos-test`, added explicitly through
System Settings after user confirmation, reported Accessibility and Input
Monitoring as `Granted`. Its mouse/keyboard indicators became active and Mos
compatibility remained enabled. Do not re-sign or move a working test copy after
granting access; future test builds may need fresh consent. On macOS 27 this
privacy pane is named “Device Control and Data Access”.

The first test package also mixed an official UI with a development backend,
which the local IPC identity check correctly rejected. Build the UI, backend and
CLI together with identical version, build and source fingerprint values, and
verify packaged client/server IPC plus an actual native window before delivery.
The build-101 replacement passed these checks. Neither this evidence nor the
two-direction wheel test establishes exhaustive coverage of every Mos option or
all permissions/audio/file-provider features.
