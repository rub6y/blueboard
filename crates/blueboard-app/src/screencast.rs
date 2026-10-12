//! A magic window's live content: screen/window capture via the
//! `org.freedesktop.portal.ScreenCast` portal + PipeWire — the Wayland-sanctioned route to
//! another window's/screen's pixels (the same mechanism Discord/Zoom/OBS use for screen
//! share), replacing this project's earlier embedded-shell and planned-but-unbuilt webview
//! magic-window strategies (see the ledger). The compositor itself pops its own native
//! source picker when a session starts — this crate never draws one, and has no visibility
//! into what's offered.
//!
//! Two phases, both run on one background thread per magic window so `ensure_live_
//! screencasts` (`main.rs`) never blocks the render loop waiting on either:
//! 1. **Portal negotiation** (`ashpd`, async-only — hence the small `tokio` runtime built
//!    just for this): create a session, select source types, `start()` it (the call that
//!    blocks on the user's picker choice), then open a PipeWire remote — yields a node id
//!    and an fd restricted to just this stream.
//! 2. **Stream consumption** (`pipewire`, synchronous/callback-driven): connect a `Stream`
//!    to that fd/node, negotiate a plain (non-DMA-BUF) RGBA/RGBx/BGRx/RGB format — v1 scope,
//!    see the ledger on why zero-copy GPU import is deferred — and push every decoded
//!    frame's RGBA8 bytes to the main thread over an `mpsc` channel. `pump` drains it into
//!    `Renderer::set_element_texture`, the exact path `gif.rs` already uses.
//!
//! Known limitation: dropping a `LiveScreencast` signals the PipeWire mainloop to quit, but
//! a thread still waiting on the user's *compositor picker dialog* (hasn't reached the
//! PipeWire stage yet) won't observe that until the portal call returns — deleting a magic
//! window element while its picker is still up can leave that thread running until the
//! user responds to it. Acceptable for a first step; not silently swept under, see the
//! ledger.

use std::sync::mpsc;

use blueboard_core::scene::ImageData;
use pipewire as pw;
use pw::properties::properties;

pub enum SpawnError {
    Runtime(String),
}

impl std::fmt::Display for SpawnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpawnError::Runtime(err) => write!(f, "failed to start capture thread: {err}"),
        }
    }
}

/// Sent to the PipeWire mainloop thread to make it quit — see this module's doc comment
/// on the one known gap in that signal reaching it promptly.
struct Terminate;

pub struct LiveScreencast {
    frame_rx: mpsc::Receiver<ImageData>,
    error_rx: mpsc::Receiver<String>,
    quit_tx: pw::channel::Sender<Terminate>,
    _thread: std::thread::JoinHandle<()>,
}

impl LiveScreencast {
    /// Returns immediately — the portal negotiation (which can take an arbitrarily long
    /// time, however long the user takes at the compositor's picker) and the PipeWire
    /// stream both run entirely on the spawned thread, never blocking the caller (this is
    /// called from the render loop's `ensure_live_screencasts`).
    pub fn spawn() -> Result<Self, SpawnError> {
        let (frame_tx, frame_rx) = mpsc::channel();
        let (error_tx, error_rx) = mpsc::channel();
        let (quit_tx, quit_rx) = pw::channel::channel::<Terminate>();
        let thread = std::thread::Builder::new()
            .name("magic-window-capture".to_string())
            .spawn(move || {
                if let Err(err) = capture_thread(frame_tx, quit_rx) {
                    let _ = error_tx.send(err);
                }
            })
            .map_err(|err| SpawnError::Runtime(err.to_string()))?;
        Ok(Self {
            frame_rx,
            error_rx,
            quit_tx,
            _thread: thread,
        })
    }

    /// Drains whatever frames have arrived since the last call, returning only the most
    /// recent one — dropping older queued frames is fine, this is a live view (not a
    /// recording) and nothing reads a skipped one. Call once per redraw.
    pub fn pump(&mut self) -> Option<ImageData> {
        self.frame_rx.try_iter().last()
    }

    /// Takes the capture thread's terminal error, if it has failed — checked once by
    /// `ensure_live_screencasts` right after a successful `spawn` wouldn't catch anything
    /// yet (the failure is necessarily asynchronous), so this is polled each redraw instead
    /// like `pump`.
    pub fn take_error(&mut self) -> Option<String> {
        self.error_rx.try_recv().ok()
    }
}

impl Drop for LiveScreencast {
    fn drop(&mut self) {
        let _ = self.quit_tx.send(Terminate);
    }
}

/// One `tokio` runtime shared by every capture's portal negotiation, built once and never
/// torn down for the rest of the process's life — *not* a fresh one per capture. `ashpd`
/// caches a single process-wide D-Bus connection in its own `static OnceLock` (confirmed in
/// its source), whose background I/O task runs on whatever runtime was active when it was
/// first created; dropping that runtime after one capture (the original, broken approach
/// here) kills that task, leaving the *second* capture's `negotiate_portal` reusing a
/// cached connection nothing is reading replies on anymore — a silent hang, not an error,
/// since the D-Bus call just never gets a response. A `multi_thread` runtime (not
/// `current_thread`) is required since multiple captures' `block_on` calls can be in
/// flight concurrently, each from its own capture thread.
fn shared_runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("failed to start shared async runtime")
    })
}

fn capture_thread(
    frame_tx: mpsc::Sender<ImageData>,
    quit_rx: pw::channel::Receiver<Terminate>,
) -> Result<(), String> {
    let (session, node_id, fd) = shared_runtime().block_on(negotiate_portal())?;
    let result = run_stream(node_id, fd, frame_tx, quit_rx);
    // Closing our local PipeWire stream (inside `run_stream`, via the mainloop quitting)
    // only tears down *our* side — it does **not** tell the portal backend the session is
    // done. Confirmed via `xdg-desktop-portal-wlr`'s own trace logging: without this
    // explicit `Close()`, its "active screencast instances" counter only ever climbed
    // (4 -> 5 -> 6 -> ...) across a session, even past elements being deleted normally —
    // every capture left a zombie session behind server-side, eventually degrading or
    // breaking later captures. Best-effort: a failure here doesn't invalidate the capture
    // that already happened, just logged.
    if let Err(err) = shared_runtime().block_on(session.close()) {
        log::warn!("failed to close magic window portal session: {err}");
    }
    result
}

async fn negotiate_portal() -> Result<
    (
        ashpd::desktop::Session<'static, ashpd::desktop::screencast::Screencast<'static>>,
        u32,
        std::os::fd::OwnedFd,
    ),
    String,
> {
    use ashpd::desktop::screencast::{CursorMode, Screencast, SourceType};
    use ashpd::desktop::PersistMode;

    let proxy = Screencast::new().await.map_err(|err| err.to_string())?;
    let session = proxy
        .create_session()
        .await
        .map_err(|err| err.to_string())?;
    // Not every portal backend supports every `CursorMode` — `xdg-desktop-portal-wlr`
    // (this machine's wlroots-compositor backend) rejects `Metadata` outright
    // ("Unavailable cursor mode 4"), so ask what's actually on offer instead of assuming
    // the richest mode works everywhere. Preference order: `Metadata` (cursor as separate
    // stream data, nicest if supported) > `Embedded` (baked into the frame, still shows a
    // cursor) > `Hidden` (always available per the portal spec, worst case).
    let available_cursor_modes = proxy
        .available_cursor_modes()
        .await
        .map_err(|err| err.to_string())?;
    let cursor_mode = [
        CursorMode::Metadata,
        CursorMode::Embedded,
        CursorMode::Hidden,
    ]
    .into_iter()
    .find(|mode| available_cursor_modes.contains(*mode))
    .unwrap_or(CursorMode::Hidden);
    proxy
        .select_sources(
            &session,
            cursor_mode,
            SourceType::Monitor | SourceType::Window,
            false,
            None,
            PersistMode::DoNot,
        )
        .await
        .map_err(|err| err.to_string())?;
    let streams = proxy
        .start(&session, None)
        .await
        .map_err(|err| err.to_string())?
        .response()
        .map_err(|err| err.to_string())?;
    let stream = streams
        .streams()
        .first()
        .ok_or("no source selected")?
        .clone();
    let fd = proxy
        .open_pipe_wire_remote(&session)
        .await
        .map_err(|err| err.to_string())?;
    Ok((session, stream.pipe_wire_node_id(), fd))
}

/// Threaded through both the `param_changed` and `process` stream callbacks (`pipewire`'s
/// `add_local_listener_with_user_data`) — the negotiated format is only known from the
/// first, needed to interpret raw bytes in the second.
struct StreamData {
    format: pw::spa::param::video::VideoInfoRaw,
    frame_tx: mpsc::Sender<ImageData>,
}

fn run_stream(
    node_id: u32,
    fd: std::os::fd::OwnedFd,
    frame_tx: mpsc::Sender<ImageData>,
    quit_rx: pw::channel::Receiver<Terminate>,
) -> Result<(), String> {
    pw::init();
    let mainloop = pw::main_loop::MainLoop::new(None).map_err(|err| err.to_string())?;
    let _quit_receiver = quit_rx.attach(mainloop.loop_(), {
        let mainloop = mainloop.clone();
        move |Terminate| mainloop.quit()
    });

    let context = pw::context::Context::new(&mainloop).map_err(|err| err.to_string())?;
    let core = context
        .connect_fd(fd, None)
        .map_err(|err| err.to_string())?;
    let stream = pw::stream::Stream::new(
        &core,
        "blueboard-magic-window",
        properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )
    .map_err(|err| err.to_string())?;

    let data = StreamData {
        format: Default::default(),
        frame_tx,
    };
    let _listener = stream
        .add_local_listener_with_user_data(data)
        .param_changed(|stream, user_data, id, param| {
            let Some(param) = param else { return };
            // The exporter (xdg-desktop-portal-wlr) asks here, separately from Format,
            // how many buffers we want — answering is not optional: left unanswered, it
            // falls back to whatever minimal default it picks (confirmed live: a stream of
            // continuous "out of buffers" / "unable to export buffer, dropping frame"
            // warnings on *every* capture, first one included, not just under heavy
            // concurrent load — the exporter had nowhere near enough buffers to pipeline
            // frames while we're still holding the previous one). Requesting a handful
            // (default 4, 2-8) gives it room.
            if id == pw::spa::param::ParamType::Buffers.as_raw() {
                match buffers_pod(4, 2, 8) {
                    Ok(pod) => {
                        if let Err(err) = stream.update_params(&mut [pod]) {
                            log::warn!("magic window: failed to request buffer count: {err}");
                        }
                    }
                    Err(err) => log::warn!("magic window: failed to build buffers pod: {err}"),
                }
                return;
            }
            if id != pw::spa::param::ParamType::Format.as_raw() {
                return;
            }
            let Ok((media_type, media_subtype)) = pw::spa::param::format_utils::parse_format(param)
            else {
                return;
            };
            if media_type != pw::spa::param::format::MediaType::Video
                || media_subtype != pw::spa::param::format::MediaSubtype::Raw
            {
                return;
            }
            if let Err(err) = user_data.format.parse(param) {
                log::warn!("magic window: failed to parse negotiated video format: {err}");
            }
        })
        .process(|stream, user_data| {
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let Some(plane) = buffer.datas_mut().first_mut() else {
                return;
            };
            let size = plane.chunk().size() as usize;
            let stride = plane.chunk().stride() as usize;
            let width = user_data.format.size().width;
            let height = user_data.format.size().height;
            let format = user_data.format.format();
            let Some(bytes) = plane.data() else {
                return;
            };
            let bytes = &bytes[..size.min(bytes.len())];
            if let Some(rgba) = convert_to_rgba(format, width, height, stride, bytes) {
                let _ = user_data.frame_tx.send(ImageData {
                    rgba,
                    width,
                    height,
                });
            }
        })
        .register()
        .map_err(|err| err.to_string())?;

    let mut params = [format_pod()?];
    stream
        .connect(
            pw::spa::utils::Direction::Input,
            Some(node_id),
            pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
            &mut params,
        )
        .map_err(|err| err.to_string())?;

    mainloop.run();
    Ok(())
}

/// Builds a `ParamBuffers` pod suggesting how many buffers the exporter should allocate —
/// answers the `Buffers`-id `param_changed` callback (`run_stream`'s listener). `libspa`
/// has no typed `ParamBuffersProperties` enum (unlike `FormatProperties`), so the `buffers`
/// key is the raw `SPA_PARAM_BUFFERS_buffers` constant from the bindgen'd `spa_sys`, used
/// directly as `pod::Property`'s `u32` key instead of going through the `property!` macro
/// (which expects a typed key with `.as_raw()`).
fn buffers_pod(default: i32, min: i32, max: i32) -> Result<&'static pw::spa::pod::Pod, String> {
    use pw::spa::pod::{object, serialize::PodSerializer, Pod, Property, PropertyFlags, Value};
    use pw::spa::utils::{Choice, ChoiceEnum, ChoiceFlags, SpaTypes};

    let obj = object!(
        SpaTypes::ObjectParamBuffers,
        pw::spa::param::ParamType::Buffers,
        Property {
            key: pw::spa::sys::SPA_PARAM_BUFFERS_buffers,
            flags: PropertyFlags::empty(),
            value: Value::Choice(pw::spa::pod::ChoiceValue::Int(Choice(
                ChoiceFlags::empty(),
                ChoiceEnum::Range { default, min, max },
            ))),
        },
    );
    let bytes = PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &Value::Object(obj))
        .map_err(|err| format!("{err:?}"))?
        .0
        .into_inner();
    let bytes: &'static [u8] = Vec::leak(bytes);
    Pod::from_bytes(bytes).ok_or_else(|| "failed to build buffers pod".to_string())
}

/// Builds the `EnumFormat` pod offering only the pixel formats `convert_to_rgba` below
/// actually handles — deliberately not listing YUV formats (I420/YUY2/etc, which real
/// compositors do also offer): converting those needs real colorspace math this first step
/// doesn't have, so they're just never on the table to negotiate. No modifier is specified,
/// which keeps negotiation on PipeWire's plain (non-DMA-BUF) buffer path — see this
/// module's doc comment on why zero-copy GPU import is deferred.
fn format_pod() -> Result<&'static pw::spa::pod::Pod, String> {
    use pw::spa::param::format::{FormatProperties, MediaSubtype, MediaType};
    use pw::spa::param::video::VideoFormat;
    use pw::spa::pod::{object, property, serialize::PodSerializer, Pod, Value};
    use pw::spa::utils::{Fraction, Rectangle, SpaTypes};

    let obj = object!(
        SpaTypes::ObjectParamFormat,
        pw::spa::param::ParamType::EnumFormat,
        property!(FormatProperties::MediaType, Id, MediaType::Video),
        property!(FormatProperties::MediaSubtype, Id, MediaSubtype::Raw),
        property!(
            FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            VideoFormat::RGBA,
            VideoFormat::RGBA,
            VideoFormat::RGBx,
            VideoFormat::BGRx,
            VideoFormat::RGB,
        ),
        property!(
            FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            Rectangle {
                width: 1280,
                height: 720,
            },
            Rectangle {
                width: 1,
                height: 1,
            },
            Rectangle {
                width: 8192,
                height: 8192
            }
        ),
        property!(
            FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            Fraction { num: 30, denom: 1 },
            Fraction { num: 0, denom: 1 },
            Fraction { num: 60, denom: 1 }
        ),
    );
    let bytes = PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &Value::Object(obj))
        .map_err(|err| format!("{err:?}"))?
        .0
        .into_inner();
    // Leaked rather than borrowed from a local: `stream.connect` takes `&mut [Pod]` with no
    // lifetime tying it to this function's caller, and this runs once per capture (not per
    // frame), so the one-time leak is the same "accept it, this isn't a hot path" call
    // `Box::leak` elsewhere in this codebase hasn't needed to make yet but fits here.
    let bytes: &'static [u8] = Vec::leak(bytes);
    Pod::from_bytes(bytes).ok_or_else(|| "failed to build format pod".to_string())
}

/// Converts one PipeWire video frame to tightly-packed RGBA8 (no stride padding), the shape
/// `Renderer::set_element_texture` expects — `None` for a format this first step doesn't
/// handle (shouldn't happen: `format_pod` above only ever offers formats this function
/// covers, but the two are independently maintained, so this stays a graceful skip rather
/// than a panic if that invariant ever drifts).
fn convert_to_rgba(
    format: pw::spa::param::video::VideoFormat,
    width: u32,
    height: u32,
    stride: usize,
    bytes: &[u8],
) -> Option<Vec<u8>> {
    use pw::spa::param::video::VideoFormat;

    let bytes_per_pixel = match format {
        VideoFormat::RGBA | VideoFormat::RGBx | VideoFormat::BGRx => 4,
        VideoFormat::RGB => 3,
        _ => return None,
    };
    let width = width as usize;
    let height = height as usize;
    let mut rgba = vec![0u8; width * height * 4];
    for row in 0..height {
        let src_row_start = row * stride;
        let Some(src_row) = bytes.get(src_row_start..src_row_start + width * bytes_per_pixel)
        else {
            break; // short final row (truncated buffer) — leave it zeroed, not a crash
        };
        let dst_row = &mut rgba[row * width * 4..(row + 1) * width * 4];
        match format {
            VideoFormat::RGBA => dst_row.copy_from_slice(src_row),
            VideoFormat::RGBx => {
                for (dst, src) in dst_row
                    .as_chunks_mut::<4>()
                    .0
                    .iter_mut()
                    .zip(src_row.as_chunks::<4>().0)
                {
                    dst[0..3].copy_from_slice(&src[0..3]);
                    dst[3] = 255;
                }
            }
            VideoFormat::BGRx => {
                for (dst, src) in dst_row
                    .as_chunks_mut::<4>()
                    .0
                    .iter_mut()
                    .zip(src_row.as_chunks::<4>().0)
                {
                    dst[0] = src[2];
                    dst[1] = src[1];
                    dst[2] = src[0];
                    dst[3] = 255;
                }
            }
            VideoFormat::RGB => {
                for (dst, src) in dst_row
                    .as_chunks_mut::<4>()
                    .0
                    .iter_mut()
                    .zip(src_row.as_chunks::<3>().0)
                {
                    dst[0..3].copy_from_slice(src);
                    dst[3] = 255;
                }
            }
            _ => unreachable!("filtered by bytes_per_pixel's match above"),
        }
    }
    Some(rgba)
}
