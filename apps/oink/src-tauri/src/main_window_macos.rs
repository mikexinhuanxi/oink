//! Main-window AppKit geometry: visible-edge resizing and native toolbar alignment.
use std::{
    cell::{Cell, RefCell},
    ptr::NonNull,
    rc::Rc,
};

use block2::RcBlock;
use objc2::{
    MainThreadMarker, Message,
    rc::Retained,
    runtime::{AnyObject, ProtocolObject},
};
use objc2_app_kit::{
    NSCursor, NSEvent, NSEventMask, NSEventType, NSView, NSWindow, NSWindowButton, NSWindowCollectionBehavior,
    NSWindowStyleMask,
};
use objc2_foundation::{NSNotification, NSNotificationCenter, NSObjectProtocol, NSPoint, NSRect, NSString};
use tauri::WebviewWindow;

thread_local! { static MONITORS: RefCell<Vec<Retained<AnyObject>>> = const { RefCell::new(Vec::new()) }; }
thread_local! { static TITLEBAR: RefCell<Option<TrafficLightObserver>> = const { RefCell::new(None) }; }

pub(super) struct TrafficLightObserver {
    token: Retained<ProtocolObject<dyn NSObjectProtocol>>,
}

impl Drop for TrafficLightObserver {
    fn drop(&mut self) {
        // SAFETY: the observer is removed on the same AppKit thread on which
        // it was installed. Removing it releases its block and retained window.
        unsafe { NSNotificationCenter::defaultCenter().removeObserver(self.token.as_ref()) };
    }
}

pub(super) fn observe_traffic_lights(native: &NSWindow) -> TrafficLightObserver {
    let _mtm = MainThreadMarker::new().expect("titlebar observer on AppKit thread");
    let window = native.retain();
    let restoring = Cell::new(false);
    let block = RcBlock::new(move |_: NonNull<NSNotification>| {
        // A frame change can itself cause an update. Only write changed frames,
        // and guard synchronous reentry so this never becomes a layout loop.
        if !restoring.replace(true) {
            restore_traffic_lights(&window);
            restoring.set(false);
        }
    });
    // A transparent Wry parent need not draw again when AppKit resets its
    // titlebar. Follow the native update instead of relying on WebView resize
    // or focus events. NSWindow posts this notification on its AppKit thread;
    // object filtering confines it to this main window. No polling or global
    // observer is needed, and fullscreen keeps its system-owned controls.
    let token = unsafe {
        NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
            Some(&NSString::from_str("NSWindowDidUpdateNotification")),
            Some(native),
            None,
            &block,
        )
    };
    TrafficLightObserver { token }
}

#[derive(Clone, Copy)]
struct Resize {
    origin: NSPoint,
    frame: NSRect,
    horizontal: i8,
    vertical: i8,
}

// Match the 24-point CSS shadow gutter and its 4-point resize overhang.
const SHADOW_GUTTER: f64 = 24.0;
const RESIZE_OVERHANG: f64 = 4.0;

/// Restore the same inset configured in shell.rs. AppKit can reset the native
/// titlebar after the transparent WebView has painted, without another Wry draw.
pub fn restore_traffic_lights(native: &NSWindow) {
    if native.styleMask().contains(NSWindowStyleMask::FullScreen) {
        return;
    }
    let Some(close) = native.standardWindowButton(NSWindowButton::CloseButton) else {
        return;
    };
    let Some(minimize) = native.standardWindowButton(NSWindowButton::MiniaturizeButton) else {
        return;
    };
    // SAFETY: these are live AppKit views on their owning thread. This is the
    // same titlebar hierarchy used by Wry's existing traffic_light_position.
    let Some(container) = (unsafe { close.superview().and_then(|view| view.superview()) }) else {
        return;
    };
    let spacing = minimize.frame().origin.x - close.frame().origin.x;
    let mut frame = container.frame();
    frame.size.height = close.frame().size.height + 24.0 + SHADOW_GUTTER;
    frame.origin.y = native.frame().size.height - frame.size.height;
    if container.frame() != frame {
        container.setFrame(frame);
    }
    for (index, button) in [
        Some(close),
        Some(minimize),
        native.standardWindowButton(NSWindowButton::ZoomButton),
    ]
    .into_iter()
    .flatten()
    .enumerate()
    {
        let mut origin = button.frame().origin;
        origin.x = 18.0 + SHADOW_GUTTER + index as f64 * spacing;
        if button.frame().origin != origin {
            button.setFrameOrigin(origin);
        }
    }
}

fn accepts_pointer(frame: NSRect, point: NSPoint, fullscreen: bool) -> bool {
    let inset = if fullscreen {
        0.0
    } else {
        SHADOW_GUTTER - RESIZE_OVERHANG
    };
    point.x >= frame.origin.x + inset
        && point.x <= frame.origin.x + frame.size.width - inset
        && point.y >= frame.origin.y + inset
        && point.y <= frame.origin.y + frame.size.height - inset
}

fn sync_mouse_region(native: &NSWindow, captured: &Cell<bool>) {
    // A normal window stays ordered out while its DOM is prepared. Mouse
    // tracking must not make that hidden window interactive.
    if !native.isVisible() {
        native.setIgnoresMouseEvents(true);
        return;
    }
    if NSEvent::pressedMouseButtons() == 0 {
        captured.set(false);
    }
    // Keep capture during a gesture begun in this window, even outside its
    // frame. A drag arriving from another app must still enter the content.
    if !captured.get() {
        let ignore = !accepts_pointer(
            native.frame(),
            NSEvent::mouseLocation(),
            native.styleMask().contains(NSWindowStyleMask::FullScreen),
        );
        if native.ignoresMouseEvents() != ignore {
            native.setIgnoresMouseEvents(ignore);
            if ignore {
                // Release our last WebKit edge cursor when the window gives up
                // mouse ownership; no native exterior cursor region remains.
                NSCursor::arrowCursor().set();
            }
        }
    }
}

pub fn prepare(window: &WebviewWindow) -> tauri::Result<()> {
    let _mtm = MainThreadMarker::new().expect("main window on AppKit thread");
    // SAFETY: retain Tauri's live NSWindow on its owning event-loop thread.
    let native = unsafe { Retained::retain(window.ns_window()?.cast::<NSWindow>()) }.expect("Tauri NSWindow");
    // NSView hitTest and CSS pointer-events run after WindowServer/AppKit's
    // native frame hit test. They cannot veto its exterior resize margin or
    // pass a nonzero-alpha CSS shadow through to another app. Gate the whole
    // NSWindow at the visible edge instead so the shadow gutter passes through
    // without changing the backing/frame or CSS shadow.
    // Both mouse monitors are needed: a local monitor cannot see re-entry once
    // the window ignores mouse events. Mouse-only global monitoring needs no
    // Accessibility/Input Monitoring grant. Do not use a global event tap.
    // Verified on macOS 26; these public APIs also exist on macOS 14.2+.
    // AppKit's exterior resize cursor regions can remain active even when
    // ignoresMouseEvents makes clicks pass through. Disabling cursor rects or
    // consuming local moves does not remove those system-owned regions. Give
    // our visible-edge tracker sole ownership of resizing instead. Use Tauri's
    // setter so Tao also restores this style after fullscreen; changing only
    // the NSWindow mask would bring the outer targets back on fullscreen exit.
    // FullScreenPrimary and the enabled green button retain native fullscreen
    // on a window without AppKit's frame-resize capability.
    window.set_resizable(false)?;
    native.setCollectionBehavior(native.collectionBehavior() | NSWindowCollectionBehavior::FullScreenPrimary);
    window.set_maximizable(true)?;
    TITLEBAR.with(|slot| *slot.borrow_mut() = Some(observe_traffic_lights(&native)));
    restore_traffic_lights(&native);
    native.setAcceptsMouseMovedEvents(true);
    let captured = Rc::new(Cell::new(false));
    sync_mouse_region(&native, &captured);
    let global_native = native.clone();
    let global_captured = captured.clone();
    let global = RcBlock::new(move |_: NonNull<NSEvent>| {
        sync_mouse_region(&global_native, &global_captured);
    });
    let mouse_updates = NSEventMask::MouseMoved
        | NSEventMask::LeftMouseDragged
        | NSEventMask::RightMouseDragged
        | NSEventMask::OtherMouseDragged
        | NSEventMask::LeftMouseUp
        | NSEventMask::RightMouseUp
        | NSEventMask::OtherMouseUp;
    let global_monitor = NSEvent::addGlobalMonitorForEventsMatchingMask_handler(mouse_updates, &global);
    let tracking = Rc::new(RefCell::new(None::<Resize>));
    let block = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
        // SAFETY: AppKit supplies a live event during this local callback.
        let event = unsafe { event.as_ref() };
        let kind = event.r#type();
        let point = NSEvent::mouseLocation();
        if matches!(
            kind,
            NSEventType::LeftMouseDown | NSEventType::RightMouseDown | NSEventType::OtherMouseDown
        ) && event.windowNumber() == native.windowNumber()
            && accepts_pointer(
                native.frame(),
                point,
                native.styleMask().contains(NSWindowStyleMask::FullScreen),
            )
        {
            captured.set(true);
        }
        sync_mouse_region(&native, &captured);
        if kind == NSEventType::LeftMouseDown {
            if event.windowNumber() != native.windowNumber()
                || native.styleMask().contains(NSWindowStyleMask::FullScreen)
            {
                return event as *const _ as *mut _;
            }
            let frame = native.frame();
            let left = frame.origin.x + SHADOW_GUTTER;
            let right = frame.origin.x + frame.size.width - SHADOW_GUTTER;
            let bottom = frame.origin.y + SHADOW_GUTTER;
            let top = frame.origin.y + frame.size.height - SHADOW_GUTTER;
            let near_x = (point.x - left).abs() <= RESIZE_OVERHANG || (point.x - right).abs() <= RESIZE_OVERHANG;
            let near_y = (point.y - bottom).abs() <= RESIZE_OVERHANG || (point.y - top).abs() <= RESIZE_OVERHANG;
            // The 16-point corner targets also cover the transparent part of
            // the superellipse. Interior toolbar/buttons never start a resize.
            let corner =
                (point.x < left + 12.0 || point.x > right - 12.0) && (point.y < bottom + 12.0 || point.y > top - 12.0);
            if !accepts_pointer(frame, point, false) || !(near_x || near_y || corner) {
                return event as *const _ as *mut _;
            }
            *tracking.borrow_mut() = Some(Resize {
                origin: point,
                frame,
                horizontal: if near_x || corner {
                    if point.x < (left + right) / 2.0 { -1 } else { 1 }
                } else {
                    0
                },
                vertical: if near_y || corner {
                    if point.y < (bottom + top) / 2.0 { -1 } else { 1 }
                } else {
                    0
                },
            });
            return std::ptr::null_mut();
        }
        if let Some(resize) = *tracking.borrow()
            && kind == NSEventType::LeftMouseDragged
        {
            let mut frame = resize.frame;
            let minimum = native.minSize();
            let dx = point.x - resize.origin.x;
            let dy = point.y - resize.origin.y;
            if resize.horizontal != 0 {
                frame.size.width = (resize.frame.size.width + dx * resize.horizontal as f64).max(minimum.width);
                if resize.horizontal < 0 {
                    frame.origin.x += resize.frame.size.width - frame.size.width;
                }
            }
            if resize.vertical != 0 {
                frame.size.height = (resize.frame.size.height + dy * resize.vertical as f64).max(minimum.height);
                if resize.vertical < 0 {
                    frame.origin.y += resize.frame.size.height - frame.size.height;
                }
            }
            native.setFrame_display(frame, true);
            return std::ptr::null_mut();
        }
        if kind == NSEventType::LeftMouseUp && tracking.borrow_mut().take().is_some() {
            return std::ptr::null_mut();
        }
        event as *const _ as *mut _
    });
    // Tao's startResizeDragging is unsupported on macOS. Keep the standard
    // titled NSWindow for traffic lights/fullscreen/Mission Control,
    // and track only the web shell's visible edges here. The native outer edges
    // are mouse-transparent. AppKit event tracking avoids asynchronous WebView IPC
    // jitter and preserves the opposite edge/minimum size during a resize.
    // Verified on macOS 26; uses public AppKit APIs available on macOS 14.2+.
    // SAFETY: block and retained NSWindow stay on AppKit's main thread.
    let monitor = unsafe {
        NSEvent::addLocalMonitorForEventsMatchingMask_handler(
            mouse_updates | NSEventMask::LeftMouseDown | NSEventMask::RightMouseDown | NSEventMask::OtherMouseDown,
            &block,
        )
    };
    MONITORS.with(|slot| *slot.borrow_mut() = global_monitor.into_iter().chain(monitor).collect());
    Ok(())
}

pub fn closed() {
    TITLEBAR.with(|slot| slot.borrow_mut().take());
    MONITORS.with(|slot| {
        for monitor in slot.borrow_mut().drain(..) {
            // SAFETY: token from this thread's AppKit local monitor registration.
            unsafe { NSEvent::removeMonitor(&monitor) };
        }
    });
}

/// The standard control's centre in top-origin WebView points (CSS pixels).
pub fn traffic_light_center(native: &NSWindow, webview: &NSView) -> Option<f64> {
    if native.styleMask().contains(NSWindowStyleMask::FullScreen) {
        return None;
    }
    restore_traffic_lights(native);
    let button = native.standardWindowButton(NSWindowButton::CloseButton)?;
    let rect = button.convertRect_toView(button.bounds(), Some(webview));
    let center = rect.origin.y + rect.size.height / 2.0;
    // Tao's traffic-light inset resizes the titlebar container; it is not the
    // visible button centre. AppKit 14 and 26 have different button frames and
    // origins (measured: 14 has a 16-point button at y=6; 26 has 14 at y=9).
    // Convert the real bounds into WKWebView coordinates rather than
    // assuming that an inset calibrated on one OS also centres the web toolbar.
    // Both normal/flipped views use logical points; no backing-scale multiplier.
    Some(if webview.isFlipped() {
        center - webview.bounds().origin.y
    } else {
        webview.bounds().origin.y + webview.bounds().size.height - center
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2_foundation::NSSize;

    fn frame() -> NSRect {
        NSRect::new(NSPoint::new(-900.0, 100.0), NSSize::new(1088.0, 728.0))
    }

    #[test]
    fn shadow_gutter_and_native_outer_resize_regions_pass_through() {
        let f = frame();
        for offset in [-4.0, 0.0, 4.0, 12.0, 19.0] {
            for p in [
                NSPoint::new(f.origin.x + offset, 400.0),
                NSPoint::new(f.origin.x + f.size.width - offset, 400.0),
                NSPoint::new(-400.0, f.origin.y + offset),
                NSPoint::new(-400.0, f.origin.y + f.size.height - offset),
            ] {
                assert!(!accepts_pointer(f, p, false), "gutter point {p:?}");
            }
        }
    }

    #[test]
    fn visible_edges_straddle_the_silhouette_by_four_points() {
        let f = frame();
        for offset in [20.0, 24.0, 28.0] {
            for p in [
                NSPoint::new(f.origin.x + offset, 400.0),
                NSPoint::new(f.origin.x + f.size.width - offset, 400.0),
                NSPoint::new(-400.0, f.origin.y + offset),
                NSPoint::new(-400.0, f.origin.y + f.size.height - offset),
            ] {
                assert!(accepts_pointer(f, p, false), "visible edge {p:?}");
            }
        }
        assert!(accepts_pointer(f, NSPoint::new(-400.0, 400.0), false));
    }

    #[test]
    fn fullscreen_has_no_shadow_gutter() {
        let f = frame();
        assert!(accepts_pointer(f, f.origin, true));
        assert!(accepts_pointer(
            f,
            NSPoint::new(f.origin.x + f.size.width, f.origin.y + f.size.height),
            true
        ));
    }
}
