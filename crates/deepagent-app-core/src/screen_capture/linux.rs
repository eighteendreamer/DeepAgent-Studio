mod portal;
mod x11;

pub(super) use portal::capture_portal;
pub(super) use x11::capture as capture_x11;

pub(super) fn is_wayland() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some_and(|display| !display.is_empty())
        || std::env::var_os("WAYLAND_SOCKET").is_some()
        || std::env::var("XDG_SESSION_TYPE").is_ok_and(|kind| kind.eq_ignore_ascii_case("wayland"))
}
