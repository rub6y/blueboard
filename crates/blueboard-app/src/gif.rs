//! Animated GIF playback for `:gif <path>`. Deliberately *not* a new `ElementKind` — the
//! placed element is a plain `ElementKind::Image` (see `commands.rs`'s `GifCommand`), so
//! selection/resize/move/save-load all work for free through machinery already built for
//! static images. Only the *animation* — which frame is showing, and when to advance — is
//! new, tracked entirely here and in `App::gif_players`, driving the same `Renderer::
//! set_element_texture` a static paste calls once, just called again each time a frame is
//! due. `.bbd` saves whatever the *first* frame was; a reopened board shows a still image,
//! not a resumed animation — same "runtime-only state, not persisted" shape `terminal.rs`
//! already established for magic windows.

use std::time::{Duration, Instant};

use blueboard_core::scene::ImageData;

pub struct GifFrame {
    pub image: ImageData,
    pub delay: Duration,
}

pub struct GifPlayer {
    frames: Vec<GifFrame>,
    current: usize,
    next_advance: Instant,
}

impl GifPlayer {
    pub fn new(frames: Vec<GifFrame>) -> Self {
        let delay = frames
            .first()
            .map(|f| f.delay)
            .unwrap_or(Duration::from_millis(100));
        Self {
            frames,
            current: 0,
            next_advance: Instant::now() + delay,
        }
    }

    /// Advances to the next frame if its delay has elapsed, returning the new frame's
    /// pixels to upload — `None` if it's not time yet. Loops back to frame 0 at the end.
    pub fn tick(&mut self) -> Option<&ImageData> {
        if Instant::now() < self.next_advance || self.frames.len() < 2 {
            return None;
        }
        self.current = (self.current + 1) % self.frames.len();
        self.next_advance = Instant::now() + self.frames[self.current].delay;
        Some(&self.frames[self.current].image)
    }
}

#[derive(Debug)]
pub enum DecodeError {
    Io(std::io::Error),
    Decode(String),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::Io(err) => write!(f, "failed to read file: {err}"),
            DecodeError::Decode(err) => write!(f, "failed to decode GIF: {err}"),
        }
    }
}

pub fn decode(path: &str) -> Result<Vec<GifFrame>, DecodeError> {
    let file = std::fs::File::open(path).map_err(DecodeError::Io)?;
    let reader = std::io::BufReader::new(file);
    let decoder = image::codecs::gif::GifDecoder::new(reader)
        .map_err(|err| DecodeError::Decode(err.to_string()))?;
    let frames = image::AnimationDecoder::into_frames(decoder)
        .collect_frames()
        .map_err(|err| DecodeError::Decode(err.to_string()))?;
    Ok(frames
        .into_iter()
        .map(|frame| {
            let delay: Duration = frame.delay().into();
            let buffer = frame.into_buffer();
            let (width, height) = buffer.dimensions();
            GifFrame {
                image: ImageData {
                    rgba: buffer.into_raw(),
                    width,
                    height,
                },
                // Some GIFs declare a zero/near-zero delay; floor it so playback doesn't
                // spin the redraw loop pointlessly fast.
                delay: delay.max(Duration::from_millis(20)),
            }
        })
        .collect())
}
