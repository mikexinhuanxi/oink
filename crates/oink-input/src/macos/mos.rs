//! Mos 4.2.1 compatibility at the existing Session capture boundary.
//!
//! Mos reverses/smooths at AnnotatedSession and posts smooth frames directly to
//! the target PID. Moving our tap behind it would lose consumed mouse wheels.
//! Mirror its direction policy on the forwarded copy instead. Local physical
//! wheels still reach Mos unchanged and retain its native smoothing pipeline.
//!
//! For received wheels, use Mos's published-source synthetic-scroll marker so
//! it does not consume, reverse or re-smooth already resolved remote input.
//! This is an implementation convention, pinned to the source documented in
//! docs/mos-compatibility.md, not an Apple event flag or a Mos public API.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Once, OnceLock, RwLock};
use std::time::Duration;

use objc2_app_kit::{NSRunningApplication, NSWorkspace};
use objc2_core_foundation::{
    CFBoolean, CFData, CFNumber, CFPreferencesAppSynchronize, CFPreferencesCopyAppValue, CFString,
};
use objc2_core_graphics::{
    CGError, CGEvent, CGEventField, CGEventSource, CGEventTapInformation, CGEventTapLocation, CGEventTapOptions,
    CGEventType, CGGetEventTapList,
};
use objc2_foundation::NSString;
use oink_foundation::input::{CapturedEvent, InputEvent, ScrollPhase, SourceKind};
use serde::Deserialize;

pub(super) const SMOOTH_MARKER: i64 = 0x4D4F53534D4F4F54;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Axes {
    x: bool,
    y: bool,
}

#[derive(Debug, Default)]
struct Snapshot {
    running: bool,
    fallback: Axes,
    by_pid: HashMap<i32, Axes>,
}

static SNAPSHOT: OnceLock<RwLock<Snapshot>> = OnceLock::new();
fn snapshot() -> &'static RwLock<Snapshot> {
    SNAPSHOT.get_or_init(|| RwLock::new(Snapshot::default()))
}

pub(super) fn init() {
    static START: Once = Once::new();
    START.call_once(|| {
        super::main_queue::run_async(refresh);
    });
}

fn refresh() {
    let next = objc2::rc::autoreleasepool(|_| read_snapshot());
    {
        let mut current = snapshot().write().unwrap_or_else(|e| e.into_inner());
        if current.running != next.running || current.fallback != next.fallback {
            tracing::info!(
                active = next.running,
                reverse_horizontal = next.fallback.x,
                reverse_vertical = next.fallback.y,
                "Mos scroll compatibility updated"
            );
        }
        *current = next;
    }
    // App enumeration and preferences IO never run on the event tap. Main
    // queues are serviced by both the desktop shell and the agent's run loop.
    super::main_queue::run_after(Duration::from_millis(500), refresh);
}

fn read_snapshot() -> Snapshot {
    let apps = NSRunningApplication::runningApplicationsWithBundleIdentifier(&NSString::from_str("com.caldis.Mos"));
    let pids: Vec<_> = (0..apps.count())
        .map(|i| apps.objectAtIndex(i))
        .filter(|app| !app.isTerminated())
        .map(|app| app.processIdentifier())
        .collect();
    if pids.is_empty() || !active_mos_tap(&pids) {
        return Snapshot::default();
    }
    let domain = CFString::from_str("com.caldis.Mos");
    CFPreferencesAppSynchronize(&domain);
    let get = |key: &str| CFPreferencesCopyAppValue(&CFString::from_str(key), &domain);
    let boolean = |key: &str, default| {
        get(key)
            .and_then(|v| {
                v.downcast_ref::<CFBoolean>()
                    .map(|b| b.as_bool())
                    .or_else(|| v.downcast_ref::<CFNumber>().and_then(|n| n.as_i64()).map(|n| n != 0))
            })
            .unwrap_or(default)
    };
    let reverse = boolean("reverse", false);
    let global = Axes {
        x: reverse && boolean("reverseHorizontal", true),
        y: reverse && boolean("reverseVertical", true),
    };
    let allowlist = boolean("allowlist", false);
    let rules: Vec<AppRule> = get("applications")
        .and_then(|v| {
            let data = v.downcast_ref::<CFData>()?;
            // Mos stores a JSON array as CFData. Ignore corrupt/future formats;
            // source-local events are never rewritten by this integration.
            (data.len() <= 1024 * 1024)
                .then(|| serde_json::from_slice(&data.to_vec()).ok())
                .flatten()
        })
        .unwrap_or_default();
    let mut next = Snapshot {
        running: true,
        fallback: if allowlist { Axes::default() } else { global },
        by_pid: HashMap::new(),
    };
    if !rules.is_empty() {
        let running = NSWorkspace::sharedWorkspace().runningApplications();
        for i in 0..running.count() {
            let app = running.objectAtIndex(i);
            let bundle = app.bundleURL().and_then(|url| url.path()).map(|p| p.to_string());
            let executable = app.executableURL().and_then(|url| url.path()).map(|p| p.to_string());
            // Mos resolves bundle paths before executable paths.
            let rule = rules
                .iter()
                .find(|r| Some(r.path.as_str()) == bundle.as_deref())
                .or_else(|| rules.iter().find(|r| Some(r.path.as_str()) == executable.as_deref()));
            if let Some(rule) = rule {
                next.by_pid.insert(app.processIdentifier(), rule.axes(global));
            }
        }
    }
    next
}

fn active_mos_tap(pids: &[i32]) -> bool {
    let empty = CGEventTapInformation {
        eventTapID: 0,
        tapPoint: CGEventTapLocation::AnnotatedSessionEventTap,
        options: CGEventTapOptions::Default,
        eventsOfInterest: 0,
        tappingProcess: 0,
        processBeingTapped: 0,
        enabled: false,
        minUsecLatency: 0.,
        avgUsecLatency: 0.,
        maxUsecLatency: 0.,
    };
    let mut taps = vec![empty; 256];
    let mut count = 0;
    // SAFETY: both output pointers reference initialized writable storage; the
    // list capacity equals max_number_of_taps. A failed query disables adoption.
    let result = unsafe { CGGetEventTapList(taps.len() as u32, taps.as_mut_ptr(), &mut count) };
    result == CGError::Success && taps.iter().take(count as usize).any(|tap| mos_tap(tap, pids))
}

fn mos_tap(tap: &CGEventTapInformation, pids: &[i32]) -> bool {
    tap.enabled
        && tap.options == CGEventTapOptions::Default
        && tap.tapPoint == CGEventTapLocation::AnnotatedSessionEventTap
        && tap.eventsOfInterest & (1 << CGEventType::ScrollWheel.0) != 0
        && pids.contains(&tap.tappingProcess)
}

fn yes() -> bool {
    true
}

#[derive(Debug, Deserialize)]
struct AppRule {
    path: String,
    #[serde(default = "yes")]
    inherit: bool,
    #[serde(default)]
    scroll: ScrollRule,
}

#[derive(Debug, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct ScrollRule {
    reverse: bool,
    reverse_horizontal: bool,
    reverse_vertical: bool,
}
impl Default for ScrollRule {
    fn default() -> Self {
        Self {
            reverse: true,
            reverse_horizontal: true,
            reverse_vertical: true,
        }
    }
}
impl AppRule {
    fn axes(&self, global: Axes) -> Axes {
        if self.inherit {
            return global;
        }
        Axes {
            x: global.x && self.scroll.reverse && self.scroll.reverse_horizontal,
            y: global.y && self.scroll.reverse && self.scroll.reverse_vertical,
        }
    }
}

pub(super) fn forwarded_scroll(event: &CapturedEvent, remote: bool, target_pid: i32) -> Cow<'_, CapturedEvent> {
    if !remote
        || event.source != SourceKind::Mouse
        || !matches!(
            event.event,
            InputEvent::Scroll {
                phase: ScrollPhase::None,
                ..
            }
        )
    {
        return Cow::Borrowed(event);
    }
    let state = snapshot().read().unwrap_or_else(|e| e.into_inner());
    let axes = if state.running {
        state.by_pid.get(&target_pid).copied().unwrap_or(state.fallback)
    } else {
        Axes::default()
    };
    apply_direction(event, axes)
}

fn apply_direction(event: &CapturedEvent, axes: Axes) -> Cow<'_, CapturedEvent> {
    if axes == Axes::default() {
        return Cow::Borrowed(event);
    }
    let mut forwarded = event.clone();
    if let InputEvent::Scroll {
        dx, dy, wheel_steps, ..
    } = &mut forwarded.event
    {
        let x = if axes.x { -1. } else { 1. };
        let y = if axes.y { -1. } else { 1. };
        *dx *= x;
        *dy *= y;
        if let Some(steps) = wheel_steps {
            steps.x *= x;
            steps.y *= y;
        }
    }
    Cow::Owned(forwarded)
}

pub(super) fn prepare_received_scroll(event: &CGEvent) {
    if CGEvent::r#type(Some(event)) == CGEventType::ScrollWheel {
        mark_received_scroll(event, snapshot().read().unwrap_or_else(|e| e.into_inner()).running);
    }
}

fn mark_received_scroll(event: &CGEvent, active: bool) {
    if active
        && CGEvent::r#type(Some(event)) == CGEventType::ScrollWheel
        && CGEvent::integer_value_field(Some(event), CGEventField::EventSourceUserData) == super::inject::MAGIC
        && CGEvent::integer_value_field(Some(event), CGEventField::EventSourceUnixProcessID)
            == i64::from(std::process::id())
    {
        // CoreGraphics caches source-owned fields after their first read.
        // Setting EventSourceUserData alone leaves the already-read Oink tag
        // unchanged on macOS. Replace a copied source, never the shared
        // injector source (which also owns keyboard and pointer events).
        if let Some(source) = CGEvent::new_source_from_event(Some(event)) {
            CGEventSource::set_user_data(Some(&source), SMOOTH_MARKER);
            CGEvent::set_source(Some(event), Some(&source));
        }
    }
}

pub(super) fn own_scroll(event: &CGEvent) -> bool {
    CGEvent::r#type(Some(event)) == CGEventType::ScrollWheel
        && CGEvent::integer_value_field(Some(event), CGEventField::EventSourceUserData) == SMOOTH_MARKER
        && CGEvent::integer_value_field(Some(event), CGEventField::EventSourceUnixProcessID)
            == i64::from(std::process::id())
}

#[cfg(test)]
mod tests {
    use super::*;
    use oink_foundation::Point;
    use oink_foundation::input::ScrollUnit;

    fn wheel() -> CapturedEvent {
        CapturedEvent {
            physical: true,
            source: SourceKind::Mouse,
            cursor: None,
            time_us: 123,
            event: InputEvent::Scroll {
                dx: 4.5,
                dy: -9.,
                unit: ScrollUnit::Line,
                phase: ScrollPhase::None,
                wheel_steps: Some(Point::new(1., -2.)),
            },
        }
    }

    #[test]
    fn source_compensation_keeps_local_event_and_reverses_both_wire_representations() {
        let original = wheel();
        for x in [false, true] {
            for y in [false, true] {
                let forwarded = apply_direction(&original, Axes { x, y });
                let InputEvent::Scroll {
                    dx, dy, wheel_steps, ..
                } = forwarded.event
                else {
                    panic!()
                };
                let sx = if x { -1. } else { 1. };
                let sy = if y { -1. } else { 1. };
                assert_eq!(
                    (dx, dy, wheel_steps),
                    (4.5 * sx, -9. * sy, Some(Point::new(sx, -2. * sy)))
                );
                assert_eq!(original, wheel());
            }
        }
    }

    #[test]
    fn local_touchpad_and_phased_input_are_never_compensated() {
        let mut event = wheel();
        assert!(matches!(forwarded_scroll(&event, false, 0), Cow::Borrowed(_)));
        event.source = SourceKind::Touchpad;
        assert!(matches!(forwarded_scroll(&event, true, 0), Cow::Borrowed(_)));
        event.source = SourceKind::Mouse;
        if let InputEvent::Scroll { phase, .. } = &mut event.event {
            *phase = ScrollPhase::Changed;
        }
        assert!(matches!(forwarded_scroll(&event, true, 0), Cow::Borrowed(_)));
    }

    #[test]
    fn app_rules_follow_global_axis_gates_and_inheritance() {
        let rule: AppRule = serde_json::from_str(
            r#"{"path":"/Example.app","inherit":false,"scroll":{"reverse":true,"reverseVertical":false}}"#,
        )
        .unwrap();
        assert_eq!(rule.axes(Axes { x: true, y: true }), Axes { x: true, y: false });
        assert_eq!(rule.axes(Axes::default()), Axes::default());
        let inherited: AppRule = serde_json::from_str(r#"{"path":"/Example.app"}"#).unwrap();
        assert_eq!(inherited.axes(Axes { x: false, y: true }), Axes { x: false, y: true });
    }

    #[test]
    fn compatible_marker_is_own_only_for_this_process_scrolls() {
        let event =
            CGEvent::new_scroll_wheel_event2(None, objc2_core_graphics::CGScrollEventUnit::Line, 2, 1, -2, 0).unwrap();
        CGEvent::set_integer_value_field(Some(&event), CGEventField::EventSourceUserData, SMOOTH_MARKER);
        for pid in [0, std::process::id(), std::process::id() + 1] {
            CGEvent::set_integer_value_field(Some(&event), CGEventField::EventSourceUnixProcessID, i64::from(pid));
            assert_eq!(own_scroll(&event), pid == std::process::id());
        }
        CGEvent::set_type(Some(&event), CGEventType::KeyDown);
        CGEvent::set_integer_value_field(
            Some(&event),
            CGEventField::EventSourceUnixProcessID,
            i64::from(std::process::id()),
        );
        assert!(!own_scroll(&event));
    }

    #[test]
    fn received_wheel_keeps_native_payload_and_only_changes_own_tag_while_mos_is_active() {
        for active in [false, true] {
            for pid in [0, std::process::id(), std::process::id() + 1] {
                let event = CGEvent::new_scroll_wheel_event2(
                    None,
                    objc2_core_graphics::CGScrollEventUnit::Pixel,
                    2,
                    -32,
                    16,
                    0,
                )
                .unwrap();
                let payload = || {
                    (
                        CGEvent::double_value_field(Some(&event), CGEventField::ScrollWheelEventPointDeltaAxis1),
                        CGEvent::double_value_field(Some(&event), CGEventField::ScrollWheelEventPointDeltaAxis2),
                        CGEvent::timestamp(Some(&event)),
                        CGEvent::flags(Some(&event)),
                        CGEvent::location(Some(&event)),
                        CGEvent::integer_value_field(Some(&event), CGEventField::EventTargetUnixProcessID),
                    )
                };
                let before = payload();
                CGEvent::set_integer_value_field(
                    Some(&event),
                    CGEventField::EventSourceUserData,
                    super::super::inject::MAGIC,
                );
                CGEvent::set_integer_value_field(Some(&event), CGEventField::EventSourceUnixProcessID, i64::from(pid));
                assert_eq!(CGEvent::r#type(Some(&event)), CGEventType::ScrollWheel);
                assert_eq!(
                    CGEvent::integer_value_field(Some(&event), CGEventField::EventSourceUnixProcessID),
                    i64::from(pid)
                );
                mark_received_scroll(&event, active);
                assert_eq!(
                    CGEvent::integer_value_field(Some(&event), CGEventField::EventSourceUserData),
                    if active && pid == std::process::id() {
                        SMOOTH_MARKER
                    } else {
                        super::super::inject::MAGIC
                    }
                );
                assert_eq!(payload(), before);
            }
        }
    }

    #[test]
    fn installed_but_paused_or_observation_only_mos_does_not_enable_compensation() {
        let mut tap = CGEventTapInformation {
            eventTapID: 1,
            tapPoint: CGEventTapLocation::AnnotatedSessionEventTap,
            options: CGEventTapOptions::Default,
            eventsOfInterest: 1 << CGEventType::ScrollWheel.0,
            tappingProcess: 100,
            processBeingTapped: 0,
            enabled: true,
            minUsecLatency: 0.,
            avgUsecLatency: 0.,
            maxUsecLatency: 0.,
        };
        assert!(mos_tap(&tap, &[100]));
        assert!(!mos_tap(&tap, &[101]));
        tap.enabled = false;
        assert!(!mos_tap(&tap, &[100]));
        tap.enabled = true;
        tap.options = CGEventTapOptions::ListenOnly;
        assert!(!mos_tap(&tap, &[100]));
        tap.options = CGEventTapOptions::Default;
        tap.eventsOfInterest = 1 << CGEventType::MouseMoved.0;
        assert!(!mos_tap(&tap, &[100]));
    }

    #[test]
    fn replacing_cached_scroll_identity_does_not_change_the_shared_injector_source() {
        let source = CGEventSource::new(objc2_core_graphics::CGEventSourceStateID::HIDSystemState).unwrap();
        CGEventSource::set_user_data(Some(&source), super::super::inject::MAGIC);
        let event =
            CGEvent::new_scroll_wheel_event2(Some(&source), objc2_core_graphics::CGScrollEventUnit::Line, 2, 1, 0, 0)
                .unwrap();
        // The Session tap has already read this tag before acknowledging it.
        assert_eq!(
            CGEvent::integer_value_field(Some(&event), CGEventField::EventSourceUserData),
            super::super::inject::MAGIC
        );
        mark_received_scroll(&event, true);
        assert!(own_scroll(&event));
        assert_eq!(CGEventSource::user_data(Some(&source)), super::super::inject::MAGIC);
    }
}
