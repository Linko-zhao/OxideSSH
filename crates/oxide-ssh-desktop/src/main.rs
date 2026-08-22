// Prevent an extra console window when launching on Windows.
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use directories::ProjectDirs;
use gpui::{App, AppContext, Application, Bounds, WindowBounds, WindowOptions, px, size};
use gpui_component::Root;
use oxide_ssh_desktop::{
    app::{self, AppView},
    app_state::AppState,
};

fn main() {
    Application::new()
        .with_assets(gpui_component_assets::Assets)
        .run(|cx: &mut App| {
            gpui_component::init(cx);
            app::init(cx);
            let root = ProjectDirs::from("io.github", "linko-zhao", "OxideSSH")
                .expect("OxideSSH requires a user configuration directory")
                .config_dir()
                .to_path_buf();
            let outcome = AppState::load(root.clone());
            let bounds = Bounds::centered(None, size(px(1080.0), px(720.0)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(800.0), px(520.0))),
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
            cx.activate(true);
        });
}
