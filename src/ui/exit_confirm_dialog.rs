use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Mutex;

use cosmic_text::{Attrs, Weight};
use niri_config::Config;
use ordered_float::NotNan;
use smithay::backend::renderer::element::utils::RescaleRenderElement;
use smithay::backend::renderer::element::Kind;
use smithay::output::Output;
use smithay::reexports::gbm::Format as Fourcc;
use smithay::utils::{Point, Transform};

use crate::animation::{Animation, Clock};
use crate::niri_render_elements;
use crate::render_helpers::memory::MemoryBuffer;
use crate::render_helpers::primary_gpu_texture::PrimaryGpuTextureRenderElement;
use crate::render_helpers::renderer::NiriRenderer;
use crate::render_helpers::solid_color::{SolidColorBuffer, SolidColorRenderElement};
use crate::render_helpers::texture::{TextureBuffer, TextureRenderElement};
use crate::ui::text_renderer::TextRenderer;
use crate::utils::{output_size, to_physical_precise_round};

const PADDING: i32 = 16;
const BORDER: i32 = 8;
const BACKDROP_COLOR: [f32; 4] = [0., 0., 0., 0.4];

pub struct ExitConfirmDialog {
    state: State,
    buffers: RefCell<HashMap<NotNan<f64>, Option<MemoryBuffer>>>,

    clock: Clock,
    config: Rc<RefCell<Config>>,
    text_renderer: *mut TextRenderer,
}

niri_render_elements! {
    ExitConfirmDialogRenderElement => {
        Texture = RescaleRenderElement<PrimaryGpuTextureRenderElement>,
        SolidColor = SolidColorRenderElement,
    }
}

struct OutputData {
    backdrop: SolidColorBuffer,
}

enum State {
    Hidden,
    Showing(Animation),
    Visible,
    Hiding(Animation),
}

impl ExitConfirmDialog {
    pub fn new(
        clock: Clock,
        config: Rc<RefCell<Config>>,
        text_renderer: *mut TextRenderer,
    ) -> Self {
        let buffer = match render(1., text_renderer) {
            Ok(x) => Some(x),
            Err(err) => {
                warn!("error creating the exit confirm dialog: {err:?}");
                None
            }
        };

        Self {
            state: State::Hidden,
            buffers: RefCell::new(HashMap::from([(NotNan::new(1.).unwrap(), buffer)])),
            clock,
            config,
            text_renderer,
        }
    }

    pub fn can_show(&self) -> bool {
        let buffers = self.buffers.borrow();
        let fallback = &buffers[&NotNan::new(1.).unwrap()];
        fallback.is_some()
    }

    fn animation(&self, from: f64, to: f64) -> Animation {
        let c = self.config.borrow();
        Animation::new(
            self.clock.clone(),
            from,
            to,
            0.,
            c.animations.exit_confirmation_open_close.0,
        )
    }

    fn value(&self) -> f64 {
        match &self.state {
            State::Hidden => 0.,
            State::Showing(anim) | State::Hiding(anim) => anim.value(),
            State::Visible => 1.,
        }
    }

    /// Returns true if the dialog will be shown (even if it is already shown).
    pub fn show(&mut self) -> bool {
        if !self.can_show() {
            return false;
        }

        if self.is_open() {
            return true;
        }

        self.state = State::Showing(self.animation(self.value(), 1.));
        true
    }

    /// Returns true if started the hide animation.
    pub fn hide(&mut self) -> bool {
        if !self.is_open() {
            return false;
        }

        self.state = State::Hiding(self.animation(self.value(), 0.));
        true
    }

    pub fn is_open(&self) -> bool {
        matches!(self.state, State::Showing(_) | State::Visible)
    }

    pub fn advance_animations(&mut self) {
        match &mut self.state {
            State::Hidden => (),
            State::Showing(anim) => {
                if anim.is_done() {
                    self.state = State::Visible;
                }
            }
            State::Visible => (),
            State::Hiding(anim) => {
                if anim.is_clamped_done() {
                    self.state = State::Hidden;
                }
            }
        }
    }

    pub fn are_animations_ongoing(&self) -> bool {
        matches!(self.state, State::Showing(_) | State::Hiding(_))
    }

    pub fn render<R: NiriRenderer>(
        &self,
        renderer: &mut R,
        output: &Output,
        push: &mut dyn FnMut(ExitConfirmDialogRenderElement),
    ) {
        let (value, clamped_value) = match &self.state {
            State::Hidden => return,
            State::Showing(anim) | State::Hiding(anim) => (anim.value(), anim.clamped_value()),
            State::Visible => (1., 1.),
        };
        let _span = tracy_client::span!("ExitConfirmDialog::render");

        // Can be out of range when starting from past 0. or 1. from a spring bounce.
        let clamped_value = clamped_value.clamp(0., 1.);

        let scale = output.current_scale().fractional_scale();
        let output_size = output_size(output);

        let mut buffers = self.buffers.borrow_mut();
        let Some(fallback) = buffers[&NotNan::new(1.).unwrap()].clone() else {
            error!("exit confirm dialog opened without fallback buffer");
            return;
        };

        let buffer = buffers
            .entry(NotNan::new(scale).unwrap())
            .or_insert_with(|| render(scale, self.text_renderer).ok());
        let buffer = buffer.as_ref().unwrap_or(&fallback);

        let size = buffer.logical_size();
        let Ok(buffer) = TextureBuffer::from_memory_buffer(renderer.as_gles_renderer(), buffer)
        else {
            return;
        };

        let location = (output_size.to_point() - size.to_point()).downscale(2.);
        let mut location = location.to_physical_precise_round(scale).to_logical(scale);
        location.x = f64::max(0., location.x);
        location.y = f64::max(0., location.y);

        let elem = TextureRenderElement::from_texture_buffer(
            buffer,
            location,
            clamped_value as f32,
            None,
            None,
            Kind::Unspecified,
        );
        let elem = PrimaryGpuTextureRenderElement(elem);
        let elem = RescaleRenderElement::from_element(
            elem,
            (location + size.downscale(2.)).to_physical_precise_round(scale),
            value.max(0.) * 0.2 + 0.8,
        );
        push(ExitConfirmDialogRenderElement::Texture(elem));

        // Backdrop.
        let data = output.user_data().get_or_insert(|| {
            Mutex::new(OutputData {
                backdrop: SolidColorBuffer::new(output_size, BACKDROP_COLOR),
            })
        });
        let mut data = data.lock().unwrap();
        data.backdrop.resize(output_size);

        let elem = SolidColorRenderElement::from_buffer(
            &data.backdrop,
            Point::new(0., 0.),
            clamped_value as f32,
            Kind::Unspecified,
        );
        push(ExitConfirmDialogRenderElement::SolidColor(elem));
    }
}

fn render(scale: f64, text_renderer: *mut TextRenderer) -> anyhow::Result<MemoryBuffer> {
    let _span = tracy_client::span!("exit_confirm_dialog::render");

    let border: f32 = to_physical_precise_round(scale, BORDER);
    let padding: f32 = to_physical_precise_round(scale, PADDING);

    let normal_attrs = Attrs::new().family(cosmic_text::Family::SansSerif);

    let key_attrs = Attrs::new()
        .family(cosmic_text::Family::Monospace)
        .weight(Weight::BOLD)
        .metadata(1);

    let spans = [
        (
            "Are you sure you want to exit niri?\n\n    Press ",
            &normal_attrs,
        ),
        (" Enter ", &key_attrs),
        (" to confirm.", &normal_attrs),
    ];

    let tr = unsafe { &mut *text_renderer };

    tr.buffer(14.0, scale as f32);

    tr.set_span(
        &spans,
        cosmic_text::Shaping::Advanced,
        cosmic_text::Align::Left,
        (Some(500.0 * scale as f32), None),
        false,
    );

    let mut pixmap = tr
        .draw_rect(
            border,
            padding,
            tiny_skia::Color::from_rgba(0.102, 0.102, 0.102, 0.933).unwrap(),
            tiny_skia::Color::from_rgba(0.330, 0.330, 1.0, 0.500).unwrap(),
        )
        .unwrap();

    let width: i32 = pixmap.width() as i32;
    let height: i32 = pixmap.height() as i32;

    tr.draw_text_with_highlight(
        &mut pixmap,
        border,
        padding,
        tiny_skia::Color::from_rgba(0.233, 0.233, 0.211, 0.933).unwrap(),
    );

    let data = pixmap.take();

    let buffer = MemoryBuffer::new(
        data,
        Fourcc::Argb8888,
        (width, height),
        scale,
        Transform::Normal,
    );

    Ok(buffer)
}

#[cfg(feature = "dbus")]
pub fn a11y_node() -> accesskit::Node {
    let mut node = accesskit::Node::new(accesskit::Role::AlertDialog);
    node.set_label("Exit niri");
    node.set_description(text(false));
    node.set_modal();
    node
}
