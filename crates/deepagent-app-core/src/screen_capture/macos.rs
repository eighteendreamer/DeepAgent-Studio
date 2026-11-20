use screencapturekit::prelude::*;
use screencapturekit::screenshot_manager::{CGImageExt, SCScreenshotManager};
use screencapturekit::shareable_content::SCShareableContentInfo;

use super::{capture_error, check_dimensions, unique_window, CoreError, Result, RgbaImage};

pub(super) fn capture(title: &str) -> Result<RgbaImage> {
    let content = SCShareableContent::create()
        .with_exclude_desktop_windows(true)
        .get()
        .map_err(|error| {
            capture_error(
                "list windows with ScreenCaptureKit (requires Screen Recording permission)",
                error,
            )
        })?;
    let target = unique_window(content.windows().into_iter().filter(|window| {
        window
            .title()
            .is_some_and(|name| name.trim().eq_ignore_ascii_case(title))
    }))?;
    let filter = SCContentFilter::create().with_window(&target).build();
    let info = SCShareableContentInfo::for_filter(&filter)
        .ok_or_else(|| CoreError::other("macOS 14 or later is required for window screenshots"))?;
    let (width, height) = info.pixel_size();
    check_dimensions(width, height)?;
    let config = SCStreamConfiguration::new()
        .with_width(width)
        .with_height(height);
    let image = SCScreenshotManager::capture_image(&filter, &config)
        .map_err(|error| capture_error("ScreenCaptureKit window capture", error))?;
    let (width, height) = (image.width() as u32, image.height() as u32);
    check_dimensions(width, height)?;
    let rgba = image
        .rgba_data()
        .map_err(|error| capture_error("read window image", error))?;
    RgbaImage::from_raw(width, height, rgba).ok_or_else(|| CoreError::other("invalid window image"))
}
