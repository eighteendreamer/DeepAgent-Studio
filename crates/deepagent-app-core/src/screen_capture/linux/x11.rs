use x11rb::connection::Connection;
use x11rb::protocol::composite::{ConnectionExt as _, Redirect};
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, ImageFormat, ImageOrder, MapState};

use super::super::{capture_error, check_dimensions, unique_window, CoreError, Result, RgbaImage};

pub(in crate::screen_capture) fn capture(title: &str) -> Result<RgbaImage> {
    let err = |error: &dyn std::fmt::Display| capture_error("X11 window capture", error);
    let (conn, index) = x11rb::connect(None).map_err(|e| err(&e))?;
    let setup = conn.setup();
    let screen = &setup.roots[index];
    let atom = |name: &[u8]| {
        conn.intern_atom(false, name)
            .ok()?
            .reply()
            .ok()
            .map(|reply| reply.atom)
    };
    let utf8 =
        atom(b"UTF8_STRING").ok_or_else(|| CoreError::other("X11 UTF8_STRING unavailable"))?;
    let name_atom =
        atom(b"_NET_WM_NAME").ok_or_else(|| CoreError::other("X11 _NET_WM_NAME unavailable"))?;
    let clients = atom(b"_NET_CLIENT_LIST")
        .and_then(|list| {
            conn.get_property(false, screen.root, list, AtomEnum::WINDOW, 0, 32768)
                .ok()
        })
        .and_then(|cookie| cookie.reply().ok())
        .and_then(|reply| reply.value32().map(|values| values.collect::<Vec<_>>()))
        .unwrap_or(
            conn.query_tree(screen.root)
                .map_err(|e| err(&e))?
                .reply()
                .map_err(|e| err(&e))?
                .children,
        );
    let window = unique_window(clients.into_iter().filter(|&window| {
        [
            (name_atom, utf8),
            (AtomEnum::WM_NAME.into(), AtomEnum::STRING.into()),
        ]
        .into_iter()
        .any(|(property, kind)| {
            conn.get_property(false, window, property, kind, 0, 512)
                .ok()
                .and_then(|cookie| cookie.reply().ok())
                .filter(|reply| reply.bytes_after == 0)
                .and_then(|reply| String::from_utf8(reply.value).ok())
                .is_some_and(|name| name.trim().eq_ignore_ascii_case(title))
        })
    }))?;
    let attributes = conn
        .get_window_attributes(window)
        .map_err(|e| err(&e))?
        .reply()
        .map_err(|e| err(&e))?;
    if attributes.map_state != MapState::VIEWABLE {
        return Err(CoreError::other(
            "restore the X11 window before taking a screenshot",
        ));
    }
    conn.composite_query_version(0, 4)
        .map_err(|e| err(&e))?
        .reply()
        .map_err(|e| err(&e))?;
    let pixmap = conn.generate_id().map_err(|e| err(&e))?;
    // Read a window-owned pixmap, never pixels from the root screen. Closing
    // this short-lived X connection releases the pixmap and any redirection.
    if conn
        .composite_name_window_pixmap(window, pixmap)
        .map_err(|e| err(&e))?
        .check()
        .is_err()
    {
        conn.composite_redirect_window(window, Redirect::AUTOMATIC)
            .map_err(|e| err(&e))?
            .check()
            .map_err(|e| err(&e))?;
        conn.composite_name_window_pixmap(window, pixmap)
            .map_err(|e| err(&e))?
            .check()
            .map_err(|e| err(&e))?;
    }
    let geom = conn
        .get_geometry(pixmap)
        .map_err(|e| err(&e))?
        .reply()
        .map_err(|e| err(&e))?;
    let (width, height) = (u32::from(geom.width), u32::from(geom.height));
    check_dimensions(width, height)?;
    let standard_rgb = screen
        .allowed_depths
        .iter()
        .flat_map(|depth| &depth.visuals)
        .any(|visual| {
            visual.visual_id == attributes.visual
                && visual.red_mask == 0xff0000
                && visual.green_mask == 0xff00
                && visual.blue_mask == 0xff
        });
    if !standard_rgb
        || !setup
            .pixmap_formats
            .iter()
            .any(|f| f.depth == geom.depth && f.bits_per_pixel == 32 && f.scanline_pad <= 32)
    {
        return Err(CoreError::other("unsupported X11 window pixel format"));
    }
    let mut pixels = conn
        .get_image(
            ImageFormat::Z_PIXMAP,
            pixmap,
            0,
            0,
            geom.width,
            geom.height,
            u32::MAX,
        )
        .map_err(|e| err(&e))?
        .reply()
        .map_err(|e| err(&e))?
        .data;
    for pixel in pixels.chunks_exact_mut(4) {
        if setup.image_byte_order == ImageOrder::LSB_FIRST {
            pixel.swap(0, 2);
            pixel[3] = 255;
        } else {
            pixel.copy_within(1..4, 0);
            pixel[3] = 255;
        }
    }
    RgbaImage::from_raw(width, height, pixels)
        .ok_or_else(|| CoreError::other("incomplete X11 window image"))
}
