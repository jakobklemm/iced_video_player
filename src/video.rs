use crate::Error;
use ffmpeg_next::format::Pixel;
use ffmpeg_next::frame::Video as FVideo;
use ffmpeg_next::Rational;
use kanal::{Receiver, Sender};
use parking_lot::Mutex;
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use tracing::{info, instrument, warn};
use video_rs::hwaccel::HardwareAccelerationDeviceType;

use video_rs::{DecoderBuilder, Location, Resize};

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

use video_rs::{Decoder, Time};

pub(crate) struct Internal {
    pub(crate) id: u64,

    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) framerate: f64,

    pub(crate) duration: Time,

    // pub timestamp: Time,
    pub timebase: Rational,

    // Really ???
    // pub(crate) frame: Arc<[u8]>,
    // pub(crate) upload_frame: Arc<AtomicBool>,
    pub shared: Arc<Shared>,
    // to notify the thread that a new frame can be drawn
    pub send: Sender<()>,

    // pub(crate) wait: mpsc::Receiver<()>,
    // pub(crate) paused: bool,
    // pub(crate) muted: bool,
    // pub(crate) looping: bool,
    // pub(crate) is_eos: bool,
    // pub(crate) restart_stream: bool,
    pub(crate) next_redraw: Instant,
}

/// TODO: See if one Mutex for all fields would still be fast enough for individiual accesses
pub struct Shared {
    pub frame: Arc<Mutex<Vec<u8>>>,
    decoder: Arc<Mutex<Decoder>>,
    pub timestamp: Arc<Mutex<Time>>,
    pub paused: AtomicBool,
    next: Receiver<()>,
    pub base: Rational,
    pub draw: AtomicBool,
}

impl Shared {
    fn run(shared: Arc<Shared>) -> JoinHandle<()> {
        thread::spawn(move || {
            let shared = shared.clone();
            loop {
                if shared.paused.load(Ordering::SeqCst) {
                    // wait on this thread before rechecking pause
                    let _ = shared.next.recv();
                } else if !shared.draw.load(Ordering::SeqCst) {
                    match shared.next() {
                        Ok(_) => {}
                        Err(_) => {
                            shared.paused.store(true, Ordering::SeqCst);
                        }
                    }
                } else {
                }
            }
        })
    }

    fn next(&self) -> Result<(), Error> {
        let mut raw = {
            let mut decoder = self.decoder.lock();
            let raw = decoder.decode_raw()?;
            raw
        };
        let pts = (*raw).pts();
        let mut scaler = raw.converter(Pixel::RGBA).unwrap();
        let mut converted = FVideo::empty();
        let _ = scaler.run(&mut raw, &mut converted).unwrap();
        {
            let time = Time::new(pts, self.base);
            let mut timestamp = self.timestamp.lock();
            *timestamp = time;
        }
        let mut frame = self.frame.lock();
        *frame = converted.data(0).to_vec();

        self.draw.store(true, Ordering::SeqCst);

        Ok(())
    }

    fn seek(&self, position: impl Into<Position>) -> Result<(), Error> {
        let mut decoder = self.decoder.lock();
        // currently not setting the timestamp, gets set at next draw call
        // let mut timestamp = self.timestamp.lock();
        match position.into() {
            Position::Time(dur) => {
                let millis = dur.as_millis();
                let int: i64 = millis.try_into()?;
                decoder.seek(int)?;
            }
            Position::Frame(frame) => {
                decoder.seek_to_frame(frame)?;
            }
        }
        Ok(())
    }
}

use std::fmt::Debug;
impl Debug for Internal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let id = self.id;
        let width = self.width;
        let height = self.height;
        let rate = self.framerate;
        let dur = self.duration;
        // let pos = self.timestamp.as_secs_f64();
        write!(
            f,
            "Internal id={id}, width={width}, height={height}, rate={rate}, duration={dur}"
        )
    }
}

impl Internal {
    pub(crate) fn seek(&mut self, position: impl Into<Position>) -> Result<(), Error> {
        self.shared.seek(position)
    }

    pub(crate) fn restart_stream(&mut self) -> Result<(), Error> {
        self.set_paused(false);
        self.shared.decoder.lock().seek_to_start()?;
        Ok(())
    }

    pub(crate) fn set_paused(&mut self, paused: bool) {
        self.shared.paused.store(paused, Ordering::SeqCst);
        let _ = self.send.send(());
    }
}

/// A multimedia video loaded from a URI (e.g., a local file path or HTTP stream).
pub struct Video(pub(crate) RefCell<Internal>);

impl Drop for Video {
    fn drop(&mut self) {
        // TODO: terminate thread
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
        // this doesn't work, because it will panic in an unimplemented()! on windows on newer
        // ffmpeg versions, cause why bother providing stable APIs?
        // let hw = HardwareAccelerationDeviceType::list_available();
        let cuda = HardwareAccelerationDeviceType::Cuda;
        let dx = HardwareAccelerationDeviceType::Dxva2;
        let mut decoder = if cuda.is_available() {
            DecoderBuilder::new(location)
                .with_resize(Resize::Fit(720, 720))
                .with_hardware_acceleration(cuda)
                .build()?
        } else if dx.is_available() {
            DecoderBuilder::new(location)
                .with_resize(Resize::Fit(720, 720))
                .with_hardware_acceleration(dx)
                .build()?
        } else {
            // if not cuda just use fallback first element
            warn!("no hardware acceleration found, video playback might not be real time");
            DecoderBuilder::new(location)
                .with_resize(Resize::Fit(720, 720))
                .build()?
        };
        let (width, height) = decoder.size_out();
        let framerate = decoder.frame_rate() as f64;
        let duration = decoder.duration()?;
        if !duration.has_value() {
            // maybe live / not real?
            return Err(Error::Unknown);
        }
        // let frame_buf = vec![0; (width * height * 4) as _];
        let mut raw = decoder.decode_raw()?;
        let mut scaler = raw.converter(Pixel::RGBA).unwrap();
        let mut converted = FVideo::empty();
        let _ = scaler.run(&mut raw, &mut converted).unwrap();

        // let frame = Arc::new(Mutex::new(frame_buf));
        let timebase = decoder.time_base();
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
        // let count = width * height * 4;
        // let frame = Vec::with_capacity(count as usize);

        // don't buffer messages
        let (snd, recv) = kanal::bounded(0);

        let shared = Shared {
            frame: Arc::new(Mutex::new(converted.data(0).to_vec())),
            decoder: Arc::new(Mutex::new(decoder)),
            timestamp: Arc::new(Mutex::new(timestamp)),
            paused: AtomicBool::new(false),
            next: recv,
            base: timebase.clone(),
            draw: upload,
        };
        let arcsh = Arc::new(shared);
        Shared::run(arcsh.clone());

        Ok(Video(RefCell::new(Internal {
            id,
            // timestamp,
            timebase,
            width,
            height,
            duration,
            send: snd,
            shared: arcsh,
            framerate,
            // paused: false,
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
    // #[inline(always)]
    // pub fn eos(&self) -> bool {
    //     self.0.borrow().is_eos
    // }

    /// Set if the media is paused or not.
    pub fn set_paused(&mut self, paused: bool) {
        let mut inner = self.0.borrow_mut();
        inner.set_paused(paused);
    }

    /// Get if the media is paused or not.
    #[inline(always)]
    pub fn paused(&self) -> bool {
        self.0.borrow().shared.paused.load(Ordering::SeqCst)
    }

    /// Jumps to a specific position in the media.
    /// The seeking is not perfectly accurate.
    pub fn seek(&mut self, position: impl Into<Position>) -> Result<(), Error> {
        self.0.borrow_mut().seek(position)
    }

    /// Get the current playback position in time.
    pub fn position(&self) -> Duration {
        let inner = self.0.borrow();
        let timestamp = inner.shared.timestamp.lock();
        (*timestamp).into()
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
