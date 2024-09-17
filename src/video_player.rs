use crate::{pipeline::VideoPrimitive, video::Video, Error, Position};
use ffmpeg_next::frame::Video as FVideo;
// use ffmpeg_next::{software::scaling::context::Context, util::frame::video::Video};
use iced::{
    advanced::{self, graphics::core::event::Status, layout, widget, Widget},
    Element,
};
use iced_wgpu::primitive::pipeline::Renderer as PrimitiveRenderer;
use parking_lot::Mutex;
use std::{marker::PhantomData, sync::atomic::Ordering};
use std::{sync::Arc, time::Duration};
use tracing::{error, info};
use video_rs::ffmpeg::{codec::Flags, format::Pixel};

/// Video player widget which displays the current frame of a [`Video`](crate::Video).
pub struct VideoPlayer<'a, Message, Theme = iced::Theme, Renderer = iced::Renderer>
where
    Renderer: PrimitiveRenderer,
{
    video: &'a Video,
    on_end_of_stream: Option<Message>,
    on_new_frame: Option<Message>,
    on_error: Option<Box<dyn Fn(&Error) -> Message + 'a>>,
    _phantom: PhantomData<(Theme, Renderer)>,
}

impl<'a, Message, Theme, Renderer> VideoPlayer<'a, Message, Theme, Renderer>
where
    Renderer: PrimitiveRenderer,
{
    /// Creates a new video player widget for a given video.
    pub fn new(video: &'a Video) -> Self {
        VideoPlayer {
            video,
            on_end_of_stream: None,
            on_new_frame: None,
            on_error: None,
            _phantom: Default::default(),
        }
    }

    /// Message to send when the video reaches the end of stream (i.e., the video ends).
    pub fn on_end_of_stream(self, on_end_of_stream: Message) -> Self {
        VideoPlayer {
            on_end_of_stream: Some(on_end_of_stream),
            ..self
        }
    }

    /// Message to send when the video receives a new frame.
    pub fn on_new_frame(self, on_new_frame: Message) -> Self {
        VideoPlayer {
            on_new_frame: Some(on_new_frame),
            ..self
        }
    }

    pub fn on_error<F>(self, on_error: F) -> Self
    where
        F: 'a + Fn(&Error) -> Message,
    {
        VideoPlayer {
            on_error: Some(Box::new(on_error)),
            ..self
        }
    }
}

impl<'a, Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for VideoPlayer<'a, Message, Theme, Renderer>
where
    Message: Clone,
    Renderer: PrimitiveRenderer,
{
    fn size(&self) -> iced::Size<iced::Length> {
        iced::Size {
            width: iced::Length::Shrink,
            height: iced::Length::Shrink,
        }
    }

    fn layout(
        &self,
        _tree: &mut widget::Tree,
        _renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let (width, height) = self.video.size();
        let (width, height) = (width as f32, height as f32);
        let size = limits.resolve(
            iced::Length::Fill,
            iced::Length::Fill,
            iced::Size::new(width, height),
        );

        // fixed aspect ratio + never exceed available size
        let size = if (size.width / size.height) > (width / height) {
            iced::Size::new(size.height * (width / height), size.height)
        } else {
            iced::Size::new(size.width, size.width * (height / width))
        };

        layout::Node::new(size)
    }

    fn draw(
        &self,
        _tree: &widget::Tree,
        renderer: &mut Renderer,
        _theme: &Theme,
        _style: &advanced::renderer::Style,
        layout: advanced::Layout<'_>,
        _cursor: advanced::mouse::Cursor,
        _viewport: &iced::Rectangle,
    ) {
        let inner = self.video.0.borrow();
        renderer.draw_pipeline_primitive(
            layout.bounds(),
            VideoPrimitive::new(
                inner.id,
                // clone is on arc
                inner.shared.frame.clone(),
                (inner.width as _, inner.height as _),
                inner.shared.draw.swap(false, Ordering::SeqCst),
            ),
        );
    }

    fn on_event(
        &mut self,
        _state: &mut widget::Tree,
        event: iced::Event,
        _layout: advanced::Layout<'_>,
        _cursor: advanced::mouse::Cursor,
        _renderer: &Renderer,
        _clipboard: &mut dyn advanced::Clipboard,
        shell: &mut advanced::Shell<'_, Message>,
        _viewport: &iced::Rectangle,
    ) -> Status {
        let mut inner = self.video.0.borrow_mut();

        if let iced::Event::Window(_, iced::window::Event::RedrawRequested(now)) = event {
            let paused = inner.shared.paused.load(Ordering::SeqCst);
            if !paused {
                let redraw_interval = 1.0 / inner.framerate;
                let until_redraw =
                    redraw_interval - (now - inner.next_redraw).as_secs_f64() % redraw_interval;
                inner.next_redraw = now + Duration::from_secs_f64(until_redraw);
                inner.next_redraw = inner.next_redraw;

                shell.request_redraw(iced::window::RedrawRequest::At(inner.next_redraw));

                if let Some(on_new_frame) = self.on_new_frame.clone() {
                    shell.publish(on_new_frame);
                }
            }
            Status::Captured
        } else {
            Status::Ignored
        }
    }
}

impl<'a, Message, Theme, Renderer> From<VideoPlayer<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a + Clone,
    Theme: 'a,
    Renderer: 'a + PrimitiveRenderer,
{
    fn from(video_player: VideoPlayer<'a, Message, Theme, Renderer>) -> Self {
        Self::new(video_player)
    }
}
