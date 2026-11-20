use std::os::fd::OwnedFd;
use std::time::{Duration, Instant};

use ashpd::desktop::screencast::{CursorMode, Screencast, SelectSourcesOptions, SourceType};
use ashpd::desktop::PersistMode;
use deepagent_tools::ToolExecutionContext;
use pipewire as pw;
use pw::spa::{
    param::{
        format::{FormatProperties, MediaSubtype, MediaType},
        format_utils,
        video::{VideoFormat, VideoInfoRaw},
        ParamType,
    },
    pod::{self, serialize::PodSerializer, Pod},
    utils::{Direction, Fraction, Rectangle, SpaTypes},
};

use super::super::{
    capture_error, check_cancelled, check_dimensions, CoreError, Result, RgbaImage,
};

pub(in crate::screen_capture) async fn capture_portal(
    context: &ToolExecutionContext,
) -> Result<RgbaImage> {
    let error = |error| capture_error("Wayland window selection", error);
    let portal = Screencast::new().await.map_err(error)?;
    if !portal
        .available_source_types()
        .await
        .map_err(error)?
        .contains(SourceType::Window)
    {
        return Err(CoreError::other(
            "this Wayland desktop portal does not support selecting a window",
        ));
    }
    let session = portal
        .create_session(Default::default())
        .await
        .map_err(error)?;
    let capture = async {
        portal
            .select_sources(
                &session,
                SelectSourcesOptions::default()
                    .set_sources(Some(SourceType::Window.into()))
                    .set_multiple(false)
                    .set_persist_mode(PersistMode::DoNot)
                    .set_cursor_mode(CursorMode::Hidden),
            )
            .await
            .map_err(error)?
            .response()
            .map_err(error)?;
        let streams = portal
            .start(&session, None, Default::default())
            .await
            .map_err(error)?
            .response()
            .map_err(error)?;
        let [window] = streams.streams() else {
            return Err(CoreError::other(
                "select exactly one window in the Wayland system dialog",
            ));
        };
        // Older portals omit the source type. The session was configured to
        // accept only windows; never broaden the requested source mask.
        if window
            .source_type()
            .is_some_and(|source| source != SourceType::Window)
        {
            return Err(CoreError::other(
                "Wayland portal returned a non-window source",
            ));
        }
        let node = window.pipe_wire_node_id();
        let fd = portal
            .open_pipe_wire_remote(&session, Default::default())
            .await
            .map_err(error)?;
        let context = context.clone();
        tokio::task::spawn_blocking(move || read_frame(fd, node, &context))
            .await
            .map_err(|error| capture_error("Wayland capture worker", error))?
    };
    let result = tokio::select! {
        result = capture => result,
        _ = context.cancelled() => Err(CoreError::other("window capture cancelled")),
    };
    let _ = tokio::time::timeout(Duration::from_millis(500), session.close()).await;
    result
}

fn read_frame(fd: OwnedFd, node: u32, context: &ToolExecutionContext) -> Result<RgbaImage> {
    use std::{cell::RefCell, rc::Rc};
    check_cancelled(context)?;
    pw::init();
    let error = |error| capture_error("Wayland PipeWire window capture", error);
    let main = pw::main_loop::MainLoopRc::new(None).map_err(error)?;
    let pw_context = pw::context::ContextRc::new(&main, None).map_err(error)?;
    let core = pw_context.connect_fd_rc(fd, None).map_err(error)?;
    let stream = pw::stream::StreamRc::new(
        core,
        "DeepAgent Window Screenshot",
        pw::properties::properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )
    .map_err(error)?;
    let output = Rc::new(RefCell::new(None));
    let _listener = stream
        .add_local_listener_with_user_data(VideoInfoRaw::default())
        .param_changed(|_, format, id, param| {
            if let Some(param) = param.filter(|_| id == ParamType::Format.as_raw()) {
                if matches!(
                    format_utils::parse_format(param),
                    Ok((MediaType::Video, MediaSubtype::Raw))
                ) {
                    let _ = format.parse(param);
                }
            }
        })
        .process({
            let main = main.clone();
            let output = Rc::clone(&output);
            move |stream, format| {
                let Some(mut buffer) = stream.dequeue_buffer() else {
                    return;
                };
                let Some(data) = buffer.datas_mut().first_mut() else {
                    return;
                };
                let size = format.size();
                if check_dimensions(size.width, size.height).is_err() {
                    return;
                }
                let chunk = data.chunk();
                let (offset, length) = (chunk.offset() as usize, chunk.size() as usize);
                let Ok(stride) = usize::try_from(chunk.stride()) else {
                    return;
                };
                let Some(bytes) = data
                    .data()
                    .and_then(|bytes| bytes.get(offset..offset.saturating_add(length)))
                else {
                    return;
                };
                if let Some(image) =
                    decode_frame(bytes, stride, size.width, size.height, format.format())
                {
                    *output.borrow_mut() = Some(image);
                    main.quit();
                }
            }
        })
        .register()
        .map_err(error)?;
    let begin = Instant::now();
    let tick = Duration::from_millis(40);
    let timer = main.loop_().add_timer({
        let main = main.clone();
        let context = context.clone();
        move |_| {
            if context.is_cancelled() || begin.elapsed() >= Duration::from_secs(5) {
                main.quit();
            }
        }
    });
    if timer
        .update_timer(Some(tick), Some(tick))
        .into_result()
        .is_err()
    {
        return Err(CoreError::other(
            "could not start the PipeWire screenshot deadline",
        ));
    }
    let format = pod::object!(
        SpaTypes::ObjectParamFormat,
        ParamType::EnumFormat,
        pod::property!(FormatProperties::MediaType, Id, MediaType::Video),
        pod::property!(FormatProperties::MediaSubtype, Id, MediaSubtype::Raw),
        pod::property!(
            FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            VideoFormat::BGRx,
            VideoFormat::BGRx,
            VideoFormat::BGRA,
            VideoFormat::RGBx,
            VideoFormat::RGBA
        ),
        pod::property!(
            FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            Rectangle {
                width: 1920,
                height: 1080
            },
            Rectangle {
                width: 1,
                height: 1
            },
            Rectangle {
                width: 8192,
                height: 8192
            }
        ),
        pod::property!(
            FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            Fraction { num: 30, denom: 1 },
            Fraction { num: 0, denom: 1 },
            Fraction { num: 240, denom: 1 }
        ),
    );
    let (bytes, _) = PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pod::Value::Object(format),
    )
    .map_err(|error| capture_error("build PipeWire format", error))?;
    let mut params = [Pod::from_bytes(bytes.get_ref())
        .ok_or_else(|| CoreError::other("invalid PipeWire format"))?];
    stream
        .connect(
            Direction::Input,
            Some(node),
            pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
            &mut params,
        )
        .map_err(error)?;
    main.run();
    check_cancelled(context)?;
    let image = output.borrow_mut().take();
    image.ok_or_else(|| CoreError::other("no supported window frame arrived from PipeWire"))
}

fn decode_frame(
    bytes: &[u8],
    stride: usize,
    width: u32,
    height: u32,
    format: VideoFormat,
) -> Option<RgbaImage> {
    let row_size = width as usize * 4;
    let alpha = matches!(format, VideoFormat::RGBA | VideoFormat::BGRA);
    let bgr = matches!(format, VideoFormat::BGRx | VideoFormat::BGRA);
    if stride < row_size
        || !matches!(
            format,
            VideoFormat::BGRx | VideoFormat::BGRA | VideoFormat::RGBx | VideoFormat::RGBA
        )
    {
        return None;
    }
    let mut pixels = Vec::with_capacity(row_size.checked_mul(height as usize)?);
    for row in 0..height as usize {
        let start = row.checked_mul(stride)?;
        for pixel in bytes
            .get(start..start.checked_add(row_size)?)?
            .chunks_exact(4)
        {
            let (red, blue) = if bgr {
                (pixel[2], pixel[0])
            } else {
                (pixel[0], pixel[2])
            };
            pixels.extend([red, pixel[1], blue, if alpha { pixel[3] } else { 255 }]);
        }
    }
    RgbaImage::from_raw(width, height, pixels)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipewire_padding_is_not_part_of_the_window_image() {
        let bytes = [10, 20, 30, 0, 99, 99, 99, 99, 40, 50, 60, 0];
        let image = decode_frame(&bytes, 8, 1, 2, VideoFormat::BGRx).unwrap();
        assert_eq!(image.into_raw(), [30, 20, 10, 255, 60, 50, 40, 255]);
        assert!(decode_frame(&bytes[..11], 8, 1, 2, VideoFormat::BGRx).is_none());
    }
}
