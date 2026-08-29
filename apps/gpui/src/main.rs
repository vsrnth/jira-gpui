#[cfg(target_os = "linux")]
mod desktop_integration;

#[cfg(any(target_os = "linux", target_os = "macos"))]
use gpui::{App, AppContext as _, Bounds, Pixels, Size, WindowBounds, px, size};

#[cfg(any(target_os = "linux", target_os = "macos"))]
const INITIAL_WINDOW_SIZE: Size<Pixels> = size(px(1240.), px(900.));

#[cfg(any(target_os = "linux", target_os = "macos"))]
const INITIAL_WINDOW_MARGIN: f32 = 24.;

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn initial_window_size(display_size: Size<Pixels>) -> Size<Pixels> {
    let available_size =
        display_size.map(|dimension| (dimension.as_f32() - 2. * INITIAL_WINDOW_MARGIN).max(1.));

    size(
        px(INITIAL_WINDOW_SIZE.width.as_f32().min(available_size.width)),
        px(INITIAL_WINDOW_SIZE
            .height
            .as_f32()
            .min(available_size.height)),
    )
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn initial_window_bounds(cx: &App) -> WindowBounds {
    let display = cx.primary_display();
    let window_size = display
        .as_ref()
        .map(|display| initial_window_size(display.visible_bounds().size))
        .unwrap_or(INITIAL_WINDOW_SIZE);

    display
        .map(|display| {
            WindowBounds::Windowed(Bounds::centered_at(
                display.visible_bounds().center(),
                window_size,
            ))
        })
        .unwrap_or_else(|| WindowBounds::centered(window_size, cx))
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn main() {
    use gpui::{WindowDecorations, WindowOptions};
    use gpui_component::{Root, TitleBar};
    use jira_gpui::{AppAssets, AppShell, startup_from_environment};

    #[cfg(target_os = "linux")]
    if desktop_integration::register_from_environment().is_err() {
        eprintln!("Jira Desk: desktop integration unavailable");
    }

    let startup = startup_from_environment();

    gpui_platform::application()
        .with_assets(AppAssets)
        .run(|cx: &mut App| {
            gpui_component::init(cx);

            let window_options = WindowOptions {
                window_bounds: Some(initial_window_bounds(cx)),
                window_decorations: Some(WindowDecorations::Client),
                app_id: Some("dev.jiradesk.JiraDesk".to_owned()),
                ..TitleBar::window_options()
            };

            cx.spawn(async move |cx| {
                cx.open_window(window_options, |window, cx| {
                    window.activate_window();
                    window.set_window_title("Jira Desk");

                    let shell = cx.new(|shell_cx| AppShell::new(startup, window, shell_cx));
                    cx.new(|cx| Root::new(shell, window, cx))
                })
                .expect("failed to open the Jira dashboard window");
            })
            .detach();
        });
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn main() {
    eprintln!("Jira Desk does not support this operating system yet.");
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests {
    use super::*;

    #[test]
    fn preserves_desired_size_when_display_has_room() {
        assert_eq!(
            initial_window_size(size(px(1920.), px(1080.))),
            size(px(1240.), px(900.))
        );
    }

    #[test]
    fn fits_window_inside_small_display_with_margins() {
        assert_eq!(
            initial_window_size(size(px(1143.), px(738.))),
            size(px(1095.), px(690.))
        );
    }

    #[test]
    fn keeps_a_nonzero_window_on_extremely_small_display() {
        assert_eq!(
            initial_window_size(size(px(32.), px(32.))),
            size(px(1.), px(1.))
        );
    }
}
