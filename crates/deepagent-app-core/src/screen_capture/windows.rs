use super::{capture_error, check_dimensions, unique_window, Result, RgbaImage};

pub(super) fn capture(title: &str) -> Result<RgbaImage> {
    let windows = xcap::Window::all().map_err(|error| capture_error("enumerate windows", error))?;
    let target = unique_window(windows.into_iter().filter(|window| {
        window
            .title()
            .is_ok_and(|name| name.trim().eq_ignore_ascii_case(title))
            && window.is_minimized().is_ok_and(|minimized| !minimized)
    }))?;
    let width = target
        .width()
        .map_err(|error| capture_error("window width", error))?;
    let height = target
        .height()
        .map_err(|error| capture_error("window height", error))?;
    check_dimensions(width, height)?;
    target
        .capture_image()
        .map_err(|error| capture_error("Windows Graphics Capture", error))
}
