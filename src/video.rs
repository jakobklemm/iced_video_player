use crate::Error;
use ffmpeg_next::format::Pixel;
use ffmpeg_next::Rational;
use iced::widget::image as img;
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};
use tracing::info;

/// Position in the media.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Position {
    /// Position based on time.
    ///
    /// Not the most accurate format for videos.
    Time(Duration),
    /// Position based on nth frame.
    Frame(i64),
}

impl From<std::time::Duration> for Position {
    fn from(t: std::time::Duration) -> Self {
        Position::Time(t)
    }
}

impl From<i64> for Position {
    fn from(f: i64) -> Self {
        Position::Frame(f)
    }
}

use video_rs::{Decoder, Location, Time};

pub(crate) struct Internal {
    pub(crate) id: u64,

    // is this really the best solution?
    pub(crate) source: Arc<Mutex<Decoder>>,

    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) framerate: f32,

    pub(crate) duration: Time,

    pub timestamp: i64,
    //pub timestamp: Duration,
    // pub timebase: Rational,

    // Really ???
    pub(crate) frame: Vec<u8>, // ideally would be Arc<Mutex<[T]>>
    pub(crate) upload_frame: Arc<AtomicBool>,

    // pub(crate) wait: mpsc::Receiver<()>,
    pub(crate) paused: bool,
    // pub(crate) muted: bool,
    // pub(crate) looping: bool,
    pub(crate) is_eos: bool,
    // pub(crate) restart_stream: bool,
    pub(crate) next_redraw: Instant,
}

impl Internal {
    pub(crate) fn seek(&self, position: impl Into<Position>) -> Result<(), Error> {
        let mut source = self.source.lock()?;
        match position.into() {
            Position::Time(dur) => {
                let millis = dur.as_millis();
                let int: i64 = millis.try_into()?;
                source.seek(int)?;
            }
            Position::Frame(frame) => {
                source.seek_to_frame(frame)?;
            }
        }
        Ok(())
    }

    pub(crate) fn restart_stream(&mut self) -> Result<(), Error> {
        self.set_paused(false);
        let mut source = self.source.lock()?;
        // self.is_eos = false;
        source.seek_to_start()?;
        Ok(())
    }

    pub(crate) fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
    }
}

/// A multimedia video loaded from a URI (e.g., a local file path or HTTP stream).
pub struct Video(pub(crate) RefCell<Internal>);

impl Drop for Video {
    fn drop(&mut self) {
        // TODO: ???
    }
}

static VIDEO_ID: AtomicU64 = AtomicU64::new(0);

impl Video {
    /// Create a new video player from a given video which loads from `uri`.
    /// Note that live sourced will report the duration to be zero.
    pub fn new(uri: &url::Url) -> Result<Self, Error> {
        // ffmpeg settings setup?
        video_rs::init()?;

        let id = VIDEO_ID.fetch_add(1, Ordering::SeqCst);
        let path: Location = uri.into();
        let mut source = Decoder::new(path)?;
        // check if maybe 'size' instead of 'size_out'
        let (width, height) = source.size_out();
        let framerate = source.frame_rate();
        let duration = source.duration()?;
        if !duration.has_value() {
            // maybe live / not real?
            return Err(Error::Unknown);
        }
        // let frame_buf = vec![0; (width * height * 4) as _];
        // let frame = source.decode_raw()?;
        // let frame = Arc::new(Mutex::new(frame_buf));
        let timestamp = 0;
        // let timebase = source.time_base();
        // let frame_ref = Arc::clone(&frame);

        info!(
            message = "creating video element",
            framerate,
            width,
            height,
            ?duration
        );

        let upload = AtomicBool::new(true);
        let count = width * height * 4;

        Ok(Video(RefCell::new(Internal {
            id,
            source: Arc::new(Mutex::new(source)),
            upload_frame: Arc::new(upload),
            timestamp,
            // timestamp: Duration::from_millis(0),
            // timebase,
            width,
            height,
            duration,
            frame: Vec::with_capacity(count as usize),
            framerate,
            paused: false,
            is_eos: false,
            next_redraw: Instant::now(),
        })))
    }

    /// Get the size/resolution of the video as `(width, height)`.
    #[inline(always)]
    pub fn size(&self) -> (u32, u32) {
        (self.0.borrow().width, self.0.borrow().height)
    }

    /// Get the framerate of the video as frames per second.
    #[inline(always)]
    pub fn framerate(&self) -> f32 {
        self.0.borrow().framerate
    }

    /// Get if the stream ended or not.
    #[inline(always)]
    pub fn eos(&self) -> bool {
        self.0.borrow().is_eos
    }

    /// Set if the media is paused or not.
    pub fn set_paused(&mut self, paused: bool) {
        let mut inner = self.0.borrow_mut();
        inner.set_paused(paused);
    }

    /// Get if the media is paused or not.
    #[inline(always)]
    pub fn paused(&self) -> bool {
        self.0.borrow().paused
    }

    /// Jumps to a specific position in the media.
    /// The seeking is not perfectly accurate.
    pub fn seek(&mut self, position: impl Into<Position>) -> Result<(), Error> {
        self.0.borrow_mut().seek(position)
    }

    /// Get the current playback position in time.
    pub fn position(&self) -> Duration {
        let inner = self.0.borrow();
        let rate = inner.framerate;
        let data = inner.timestamp as f32;
        let time = data / rate;
        Duration::from_secs_f32(time)
    }

    /// Get the media duration.
    #[inline(always)]
    pub fn duration(&self) -> Time {
        self.0.borrow().duration
    }
}
