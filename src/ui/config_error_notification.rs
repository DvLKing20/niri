use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use cosmic_text::{Attrs, Weight};
use niri_config::Config;
use ordered_float::NotNan;
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::output::Output;
use smithay::reexports::gbm::Format as Fourcc;
use smithay::utils::{Point, Transform};
use tiny_skia::Color;

use crate::animation::{Animation, Clock};
use crate::render_helpers::primary_gpu_texture::PrimaryGpuTextureRenderElement;
use crate::render_helpers::renderer::NiriRenderer;
use crate::render_helpers::texture::{TextureBuffer, TextureRenderElement};
use crate::ui::text_renderer::TextRenderer;
use crate::utils::{output_size, to_physical_precise_round};

const PADDING: i32 = 8;
const BORDER: i32 = 4;

pub struct ConfigErrorNotification {
    state: State,
    buffers: RefCell<HashMap<NotNan<f64>, Option<TextureBuffer<GlesTexture>>>>,

    // If set, this is a "Created config at {path}" notification. If unset, this is a config error
    // notification.
    created_path: Option<PathBuf>,

    clock: Clock,
    config: Rc<RefCell<Config>>,
    text_renderer: *mut TextRenderer,
}

enum State {
    Hidden,
    Showing(Animation),
    Shown(Duration),
    Hiding(Animation),
}

impl ConfigErrorNotification {
    pub fn new(
        clock: Clock,
        config: Rc<RefCell<Config>>,
        text_renderer: *mut TextRenderer,
    ) -> Self {
        Self {
            state: State::Hidden,
            buffers: RefCell::new(HashMap::new()),
            created_path: None,
            clock,
            config,
            text_renderer,
        }
    }

    fn animation(&self, from: f64, to: f64) -> Animation {
        let c = self.config.borrow();
        Animation::new(
            self.clock.clone(),
            from,
            to,
            0.,
            c.animations.config_notification_open_close.0,
        )
    }

    pub fn show_created(&mut self, created_path: &Path) {
        if self.created_path.as_deref() != Some(created_path) {
            self.created_path = Some(created_path.to_owned());
            self.buffers.borrow_mut().clear();
        }

        self.state = State::Showing(self.animation(0., 1.));
    }

    pub fn show(&mut self) {
        let c = self.config.borrow();
        if c.config_notification.disable_failed {
            return;
        }

        if self.created_path.is_some() {
            self.created_path = None;
            self.buffers.borrow_mut().clear();
        }

        // Show from scratch even if already showing to bring attention.
        self.state = State::Showing(self.animation(0., 1.));
    }

    pub fn hide(&mut self) {
        if matches!(self.state, State::Hidden) {
            return;
        }

        self.state = State::Hiding(self.animation(1., 0.));
    }

    pub fn advance_animations(&mut self) {
        match &mut self.state {
            State::Hidden => (),
            State::Showing(anim) => {
                if anim.is_done() {
                    let duration = if self.created_path.is_some() {
                        // Make this quite a bit longer because it comes with a monitor modeset
                        // (can take a while) and an important hotkeys popup diverting the
                        // attention.
                        Duration::from_secs(8)
                    } else {
                        Duration::from_secs(4)
                    };
                    self.state = State::Shown(self.clock.now_unadjusted() + duration);
                }
            }
            State::Shown(deadline) => {
                if self.clock.now_unadjusted() >= *deadline {
                    self.hide();
                }
            }
            State::Hiding(anim) => {
                if anim.is_clamped_done() {
                    self.state = State::Hidden;
                }
            }
        }
    }

    pub fn are_animations_ongoing(&self) -> bool {
        !matches!(self.state, State::Hidden)
    }

    pub fn render<R: NiriRenderer>(
        &self,
        renderer: &mut R,
        output: &Output,
    ) -> Option<PrimaryGpuTextureRenderElement> {
        if matches!(self.state, State::Hidden) {
            return None;
        }

        let scale = output.current_scale().fractional_scale();
        let output_size = output_size(output);
        let path = self.created_path.as_deref();

        let mut buffers = self.buffers.borrow_mut();
        let buffer = buffers
            .entry(NotNan::new(scale).unwrap())
            .or_insert_with(move || {
                render(renderer.as_gles_renderer(), scale, path, self.text_renderer).ok()
            });
        let buffer = buffer.clone()?;

        let size = buffer.logical_size();
        let y_range = size.h + f64::from(PADDING) * 2.;

        let x = (output_size.w - size.w).max(0.) / 2.;
        let y = match &self.state {
            State::Hidden => unreachable!(),
            State::Showing(anim) | State::Hiding(anim) => -size.h + anim.value() * y_range,
            State::Shown(_) => f64::from(PADDING) * 2.,
        };

        let location = Point::from((x, y));
        let location = location.to_physical_precise_round(scale).to_logical(scale);

        let elem = TextureRenderElement::from_texture_buffer(
            buffer,
            location,
            1.,
            None,
            None,
            Kind::Unspecified,
        );
        Some(PrimaryGpuTextureRenderElement(elem))
    }
}

fn render(
    renderer: &mut GlesRenderer,
    scale: f64,
    created_path: Option<&Path>,
    text_renderer: *mut TextRenderer,
) -> anyhow::Result<TextureBuffer<GlesTexture>> {
    let _span = tracy_client::span!("config_error_notification::render");

    let padding: f32 = to_physical_precise_round(scale, PADDING);
    let border: f32 = to_physical_precise_round(scale, BORDER);

    let attrs_normal = Attrs::new().family(cosmic_text::Family::SansSerif);
    let attrs_key = Attrs::new()
        .family(cosmic_text::Family::Monospace)
        .weight(Weight::BOLD)
        .metadata(1);

    let path: String;

    let (spans, border_color) = if let Some(p) = created_path {
        path = format!(" {:?} ", p);

        (
            [
                ("Created a default ", &attrs_normal),
                ("config file at ", &attrs_normal),
                (&path, &attrs_key),
            ],
            tiny_skia::Color::from_rgba8(255, 77, 77, 255),
        )
    } else {
        (
            [
                (
                    "Failed to parse the config file. Please run ",
                    &attrs_normal,
                ),
                (" niri validate ", &attrs_key),
                (" to see the errors.", &attrs_normal),
            ],
            tiny_skia::Color::from_rgba8(100, 100, 240, 255),
        )
    };

    let font: f32 = 14.0;

    let tr = unsafe { &mut *text_renderer };
    tr.buffer(font, scale as f32);

    tr.set_span(
        &spans,
        cosmic_text::Shaping::Advanced,
        cosmic_text::Align::Center,
        (None, None),
        false,
    );

    let mut pixmap = tr
        .draw_rect(
            border,
            padding,
            Color::from_rgba(0.102, 0.102, 0.102, 0.933).unwrap(),
            border_color,
        )
        .unwrap();

    let width = pixmap.width() as i32;
    let height = pixmap.height() as i32;

    tr.draw_text_with_highlight(
        &mut pixmap,
        border,
        padding,
        Color::from_rgba(0.244, 0.233, 0.211, 0.500).unwrap(),
    );

    let data = pixmap.take();

    let buffer = TextureBuffer::from_memory(
        renderer,
        &data,
        Fourcc::Argb8888,
        (width, height),
        false,
        scale,
        Transform::Normal,
        Vec::new(),
    )?;

    Ok(buffer)
}
