#[cfg(target_os = "linux")]
fn main() {
    use gpui::{App, AppContext as _, WindowBounds, WindowOptions, px, size};
    use gpui_component::{ActiveTheme as _, Root};
    use jira_gpui::Dashboard;

    gpui_platform::application().run(|cx: &mut App| {
        gpui_component::init(cx);

        let window_options = WindowOptions {
            window_bounds: Some(WindowBounds::centered(size(px(1240.), px(780.)), cx)),
            ..WindowOptions::default()
        };

        cx.spawn(async move |cx| {
            cx.open_window(window_options, |window, cx| {
                window.activate_window();
                window.set_window_title("Jira Desk");

                let dashboard = cx.new(|_| Dashboard::from_sample_data());
                cx.new(|cx| Root::new(dashboard, window, cx).bg(cx.theme().background))
            })
            .expect("failed to open the Jira dashboard window");
        })
        .detach();
    });
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("Jira Desk Phase 1 runs on Linux/Wayland; macOS support is planned for Phase 2.");
}
