// Prevent an extra console window when launching on Windows.
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use std::path::PathBuf;

use directories::ProjectDirs;
use gpui::{App, AppContext, Application, Bounds, WindowBounds, WindowOptions, px, size};
use gpui_component::Root;
use oxide_ssh_desktop::{
    app::{self, AppView, NewWindow},
    state::AppState,
};

fn open_window(root: PathBuf, cx: &mut App) {
    let outcome = AppState::load(root.clone());
    let bounds = Bounds::centered(None, size(px(1080.0), px(720.0)), cx);
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(800.0), px(520.0))),
            // Linux keeps server-side decorations; the custom TitleBar
            // is only used where the system title bar is transparent.
            #[cfg(not(target_os = "linux"))]
            titlebar: Some(gpui_component::TitleBar::title_bar_options()),
            ..Default::default()
        },
        move |window, cx| {
            window.set_window_title("OxideSSH");
            let view = cx.new(|cx| AppView::new(root, outcome, window, cx));
            let weak = view.downgrade();
            window.on_window_should_close(cx, move |_, cx| {
                weak.update(cx, |view, cx| {
                    if view.has_live_tabs() {
                        view.request_quit(cx);
                        false
                    } else {
                        true
                    }
                })
                .unwrap_or(true)
            });
            cx.new(|cx| Root::new(view, window, cx))
        },
    )
    .expect("failed to open OxideSSH window");
}

/// Single-workspace model: "New Window" focuses the existing window and only
/// actually opens one when none exists (dock click on a windowless app).
fn open_window_unless_exists(root: PathBuf, cx: &mut App) {
    if let Some(handle) = cx.windows().first() {
        let _ = handle.update(cx, |_, window, _| window.activate_window());
    } else {
        open_window(root, cx);
    }
}
/// Sets the Dock icon only when running as a bare binary (cargo build/run).
/// Inside a packaged .app the bundle's icns already supplies the Dock icon;
/// overriding it with the PNG would bypass the bundle's sized icon set.
#[cfg(target_os = "macos")]
fn set_dock_icon() {
    use objc2::{AnyThread, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::{NSBundle, NSData};

    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    // Packaged apps have a bundle identifier; bare binaries do not.
    if NSBundle::mainBundle().bundleIdentifier().is_some() {
        return;
    }
    let png = include_bytes!("../../../assets/oxide-ssh.png");
    let data = NSData::from_vec(png.to_vec());
    let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) else {
        return;
    };
    // SAFETY: called once on the main thread inside the GPUI app callback,
    // after NSApplication is initialized.
    unsafe {
        NSApplication::sharedApplication(mtm).setApplicationIconImage(Some(&image));
    }
}

fn main() {
    let root = ProjectDirs::from("io.github", "linko-zhao", "OxideSSH")
        .expect("OxideSSH requires a user configuration directory")
        .config_dir()
        .to_path_buf();
    let app = Application::new().with_assets(gpui_component_assets::Assets);
    {
        let root = root.clone();
        app.on_reopen(move |cx| open_window_unless_exists(root.clone(), cx));
    }
    app.run(|cx: &mut App| {
        #[cfg(target_os = "macos")]
        set_dock_icon();
        gpui_component::init(cx);
        app::init(cx);
        cx.on_action({
            let root = root.clone();
            move |_: &NewWindow, cx| open_window_unless_exists(root.clone(), cx)
        });
        open_window(root, cx);
        cx.activate(true);
    });
}
