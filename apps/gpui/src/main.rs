#[cfg(target_os = "linux")]
fn main() {
    use gpui::{
        App, AppContext as _, Styled as _, WindowBounds, WindowDecorations, WindowOptions, px, size,
    };
    use gpui_component::{ActiveTheme as _, Root, TitleBar};
    use gpui_component_assets::Assets;
    use jira_gpui::{AppShell, startup_from_environment};

    let startup = startup_from_environment();

    gpui_platform::application()
        .with_assets(Assets)
        .run(|cx: &mut App| {
            gpui_component::init(cx);

            let window_options = WindowOptions {
                window_bounds: Some(WindowBounds::centered(size(px(1240.), px(780.)), cx)),
                window_decorations: Some(WindowDecorations::Client),
                app_id: Some("dev.jiradesk.JiraDesk".to_owned()),
                ..TitleBar::window_options()
            };

            cx.spawn(async move |cx| {
                cx.open_window(window_options, |window, cx| {
                    window.activate_window();
                    window.set_window_title("Jira Desk");

                    let shell = cx.new(|shell_cx| AppShell::new(startup, window, shell_cx));
                    cx.new(|cx| Root::new(shell, window, cx).bg(cx.theme().background))
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
