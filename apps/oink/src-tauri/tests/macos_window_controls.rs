//! Runs AppKit on the executable's main thread; never shows a window or posts input.
#[cfg(target_os = "macos")]
#[allow(dead_code)]
#[path = "../src/main_window_macos.rs"]
mod controls;

#[cfg(target_os = "macos")]
fn main() {
    use objc2::{MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{NSApplication, NSBackingStoreType, NSWindow, NSWindowButton, NSWindowStyleMask};
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

    let mtm = MainThreadMarker::new().expect("AppKit test on main thread");
    let _app = NSApplication::sharedApplication(mtm);
    let style = NSWindowStyleMask::Titled
        | NSWindowStyleMask::Closable
        | NSWindowStyleMask::Miniaturizable
        | NSWindowStyleMask::Resizable
        | NSWindowStyleMask::FullSizeContentView;
    let make_window = || unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            NSRect::new(NSPoint::new(100.0, 100.0), NSSize::new(1088.0, 728.0)),
            style,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    let window = make_window();
    let other = make_window();
    let close = window.standardWindowButton(NSWindowButton::CloseButton).unwrap();
    // Reproduce the measured reset explicitly: hidden-window AppKit does not
    // reset on every OS/style mutation, while the real WebView did reset later.
    let reset_layout = || {
        let container = unsafe { close.superview().unwrap().superview().unwrap() };
        let mut frame = container.frame();
        frame.size.height = 32.0;
        frame.origin.y = window.frame().size.height - frame.size.height;
        container.setFrame(frame);
        for (index, kind) in [
            NSWindowButton::CloseButton,
            NSWindowButton::MiniaturizeButton,
            NSWindowButton::ZoomButton,
        ]
        .into_iter()
        .enumerate()
        {
            let button = window.standardWindowButton(kind).unwrap();
            button.setFrameOrigin(NSPoint::new(9.0 + index as f64 * 20.0, 9.0));
        }
    };
    controls::restore_traffic_lights(&window);
    reset_layout();
    window.update();
    assert_eq!(
        close.frame().origin.x,
        9.0,
        "one-time restoration must reproduce the regression"
    );
    let observer = controls::observe_traffic_lights(&window);
    controls::restore_traffic_lights(&window);
    let expected = close.convertRect_toView(close.bounds(), window.contentView().as_deref());
    assert_eq!(expected.origin.x, 42.0);
    assert!(expected.origin.y >= 24.0, "controls must be inside the visible shell");

    for iteration in 0..20 {
        // Both are real AppKit operations that may reset the titlebar without
        // any WebView resize/focus event, as happened after build 102 startup.
        window.setTitle(&NSString::from_str(&format!("Title {iteration}")));
        window.setStyleMask(if iteration % 2 == 0 {
            style & !NSWindowStyleMask::Resizable
        } else {
            style
        });
        let mut frame = window.frame();
        frame.size.width += 1.0;
        window.setFrame_display(frame, false);
        reset_layout();
        window.update();
        let actual = close.convertRect_toView(close.bounds(), window.contentView().as_deref());
        assert_eq!(actual.origin.x, expected.origin.x, "reset {iteration}");
        assert_eq!(actual.origin.y, expected.origin.y, "reset {iteration}");
        // Repeated notifications are idempotent and do not accumulate offsets.
        window.update();
        assert_eq!(
            close.convertRect_toView(close.bounds(), window.contentView().as_deref()),
            actual
        );
    }

    let other_close = other.standardWindowButton(NSWindowButton::CloseButton).unwrap();
    let before = other_close.frame();
    other.update();
    assert_eq!(other_close.frame(), before, "observer must not affect other windows");

    drop(observer);
    reset_layout();
    window.update();
    assert_ne!(
        close.frame().origin.x,
        42.0,
        "dropping observer must stop restoring controls"
    );
    println!("Native titlebar lifecycle passed: 20 resets, idempotence, window isolation and observer removal");
}

#[cfg(not(target_os = "macos"))]
fn main() {}
