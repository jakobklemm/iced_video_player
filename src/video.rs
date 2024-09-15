use crate::Error;
use ffmpeg_next::format::Pixel;
use ffmpeg_next::frame::Video as FVideo;
use ffmpeg_next::Rational;
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};
use tracing::{info, instrument};

pub use video_rs::Location;

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
    //pub(crate) source: Arc<Mutex<Decoder>>,
    pub(crate) source: Decoder,

    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) framerate: f64,

    pub(crate) duration: Time,

    pub timestamp: Time,
    pub timebase: Rational,

    // Really ???
    pub(crate) frame: Arc<[u8]>,
    pub(crate) upload_frame: Arc<AtomicBool>,

    // pub(crate) wait: mpsc::Receiver<()>,
    pub(crate) paused: bool,
    // pub(crate) muted: bool,
    // pub(crate) looping: bool,
    pub(crate) is_eos: bool,
    // pub(crate) restart_stream: bool,
    pub(crate) next_redraw: Instant,
}

use std::fmt::Debug;
impl Debug for Internal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let id = self.id;
        let width = self.width;
        let height = self.height;
        let rate = self.framerate;
        let dur = self.duration;
        let pos = self.timestamp.as_secs_f64();
        write!(f, "Internal id={id}, width={width}, height={height}, rate={rate}, duration={dur}, timestamp={pos}")
    }
}

impl Internal {
    pub(crate) fn seek(&mut self, position: impl Into<Position>) -> Result<(), Error> {
        match position.into() {
            Position::Time(dur) => {
                let millis = dur.as_millis();
                let int: i64 = millis.try_into()?;
                self.source.seek(int)?;
            }
            Position::Frame(frame) => {
                self.source.seek_to_frame(frame)?;
            }
        }
        Ok(())
    }

    pub(crate) fn restart_stream(&mut self) -> Result<(), Error> {
        self.set_paused(false);
        // self.is_eos = false;
        self.source.seek_to_start()?;
        Ok(())
    }

    pub(crate) fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
    }

    pub(crate) fn next_frame(&mut self) -> Result<(), Error> {
        let start = Instant::now();
        let mut frame = self.source.decode_raw()?;
        let pts = frame.pts();
        let mut scaler = frame.converter(Pixel::RGBA).unwrap();
        let mut converted = FVideo::empty();
        let _ = scaler.run(&mut frame, &mut converted).unwrap();
        let time = Time::new(pts, self.timebase);
        self.timestamp = time;
        // TODO: try if &[u8] is possible
        self.frame = converted.data(0).to_vec().into();
        self.upload_frame.swap(true, Ordering::SeqCst);
        let end = Instant::now();
        let dur = end - start;
        let mil = dur.as_millis();
        let dur = dur.as_secs_f64();
        info!(message = "new frame", time = ?dur, millis = ?mil);
        // println!("frame time: {:?}, millis: {:?}", dur, mil);
        Ok(())
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
    #[instrument]
    pub fn new(location: &Location) -> Result<Self, Error> {
        // ffmpeg settings setup?
        video_rs::init()?;

        let id = VIDEO_ID.fetch_add(1, Ordering::SeqCst);
        let source = Decoder::new(location)?;
        let (width, height) = source.size_out();
        let framerate = source.frame_rate() as f64;
        let duration = source.duration()?;
        if !duration.has_value() {
            // maybe live / not real?
            return Err(Error::Unknown);
        }
        // let frame_buf = vec![0; (width * height * 4) as _];
        // let frame = source.decode_raw()?;
        // let frame = Arc::new(Mutex::new(frame_buf));
        let timebase = source.time_base();
        let timestamp = Time::new(None, timebase.clone());

        info!(
            message = "creating video element",
            id,
            framerate,
            width,
            height,
            ?duration
        );

        let upload = AtomicBool::new(true);
        let count = width * height * 4;
        let frame = Vec::with_capacity(count as usize);

        Ok(Video(RefCell::new(Internal {
            id,
            source,
            upload_frame: Arc::new(upload),
            timestamp,
            timebase,
            width,
            height,
            duration,
            frame: frame.into(),
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
    pub fn framerate(&self) -> f64 {
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
        inner.timestamp.into()
    }

    /// Get the media duration.
    #[inline(always)]
    pub fn duration(&self) -> Duration {
        let dur: Duration = self.0.borrow().duration.into();
        let fl = dur.as_secs_f64();
        let round = fl.round();
        Duration::from_secs_f64(round)
    }
}
