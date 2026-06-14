use cosmic_text::{Align, Attrs, Buffer, FontSystem, Metrics, Shaping, SwashCache};
use tiny_skia::{Color, Paint, PathBuilder, Pixmap, Rect, Stroke, Transform};

pub struct TextRenderer {
    pub font_system: FontSystem,
    swash_cache: SwashCache,
    pub buffer: Option<Buffer>,
    scale: Option<f32>,
}

impl Default for TextRenderer {
    fn default() -> Self {
        Self {
            font_system: FontSystem::new(),
            swash_cache: SwashCache::new(),
            buffer: None,
            scale: None,
        }
    }
}

impl TextRenderer {
    pub fn buffer(&mut self, font_size: f32, scale: f32) {
        if self.scale.is_none() {
            self.scale = Some(scale);

            if self.buffer.is_none() {
                let metrics = Metrics::new(font_size * scale, font_size * scale);
                let buffer = Buffer::new(&mut self.font_system, metrics);
                self.buffer = Some(buffer);
                self.buffer.as_mut().unwrap().set_metrics(metrics);
            }
        }

        if self.scale.is_some() && self.scale.unwrap() != scale {
            self.swash_cache.image_cache.clear();
            self.swash_cache.outline_command_cache.clear();
            let metrics = Metrics::new(font_size * scale, font_size * scale);
            self.buffer.as_mut().unwrap().set_metrics(metrics);
            self.scale = Some(scale);
        }
    }

    pub fn set_text(
        &mut self,
        text: &str,
        align: Align,
        shaping: Shaping,
        scroll: bool,
        wrap: (Option<f32>, Option<f32>),
        attrs: &Attrs,
    ) {
        if let Some(buffer) = &mut self.buffer {
            buffer.set_text(text, attrs, shaping, Some(align));
            let (w, h) = wrap;
            buffer.set_size(w, h);
            buffer.shape_until_scroll(&mut self.font_system, scroll);
        }
    }

    pub fn set_span(
        &mut self,
        spans: &[(&str, &Attrs)],
        shaping: Shaping,
        align: Align,
        wrap: (Option<f32>, Option<f32>),
        scroll: bool,
    ) {
        if let Some(buffer) = &mut self.buffer {
            let rich_spans = spans.iter().map(|&(text, attrs)| (text, attrs.clone()));
            buffer.set_rich_text(rich_spans, &Attrs::new(), shaping, Some(align));
            let (w, h) = wrap;
            buffer.set_size(w, h);
            buffer.shape_until_scroll(&mut self.font_system, scroll);
        }
    }

    pub fn draw_empy_rect(&self) -> Option<Pixmap> {
        let Some(buffer) = &self.buffer else {
            return None;
        };

        let mut text_w = 0.0f32;
        let mut text_h = 0.0f32;

        for run in buffer.layout_runs() {
            text_w = text_w.max(run.line_w);
            text_h = text_h.max(run.line_height + run.line_top);
        }

        let box_w = text_w;
        let box_h = text_h;

        Pixmap::new(box_w as u32, box_h as u32)
    }

    pub fn draw_rect(
        &self,
        border: f32,
        padding: f32,
        bg_color: Color,
        bd_color: Color,
    ) -> Option<Pixmap> {
        let Some(buffer) = &self.buffer else {
            return None;
        };

        let mut text_w = 0.0f32;
        let mut text_h = 0.0f32;

        for run in buffer.layout_runs() {
            text_w = text_w.max(run.line_w);
            text_h = text_h.max(run.line_height + run.line_top);
        }

        let box_w = text_w + (padding * 2.0) + (border * 2.0);
        let box_h = text_h + (padding * 2.0) + (border * 2.0);

        let mut pixmap = Pixmap::new(box_w as u32, box_h as u32).unwrap();

        let mut paint = Paint::default();
        paint.set_color(bg_color);
        paint.anti_alias = false;

        let bg_rect = Rect::from_xywh(0.0, 0.0, box_w, box_h).unwrap();

        pixmap.fill_rect(bg_rect, &paint, Transform::identity(), None);

        let stroke = Stroke {
            width: border,
            ..Default::default()
        };

        let border_rect =
            Rect::from_xywh(border / 2.0, border / 2.0, box_w - border, box_h - border).unwrap();

        paint.set_color(bd_color);
        let border_path = PathBuilder::from_rect(border_rect);

        pixmap.stroke_path(&border_path, &paint, &stroke, Transform::identity(), None);

        Some(pixmap)
    }

    pub fn draw_text(&mut self, pixmap: &mut Pixmap, border: f32, padding: f32) {
        let Some(buffer) = &mut self.buffer else {
            return;
        };

        let mut paint = Paint::default();

        buffer.draw(
            &mut self.font_system,
            &mut self.swash_cache,
            cosmic_text::Color::rgb(255, 255, 255),
            |x, y, w, h, color| {
                let x = x as f32 + border + padding;
                let y = y as f32 + border + padding;

                paint.set_color_rgba8(color.r(), color.g(), color.b(), color.a());

                pixmap.fill_rect(
                    Rect::from_xywh(x, y, w as f32, h as f32).unwrap(),
                    &paint,
                    Transform::identity(),
                    None,
                );
            },
        );
    }

    pub fn draw_text_with_highlight(
        &mut self,
        pixmap: &mut Pixmap,
        border: f32,
        padding: f32,
        color: Color,
    ) {
        let Some(buffer) = &mut self.buffer else {
            return;
        };

        let mut paint = Paint {
            anti_alias: false,
            ..Default::default()
        };

        paint.set_color(color);

        for run in buffer.layout_runs() {
            let mut bg_start: Option<f32> = None;
            let mut bg_end = 0.0f32;

            for glyph in run.glyphs.iter() {
                if glyph.metadata == 1 {
                    if bg_start.is_none() {
                        bg_start = Some(glyph.x);
                    }
                    bg_end = glyph.x + glyph.w;
                }
            }

            if let Some(raw_x) = bg_start {
                let x = raw_x + border + padding;
                let y = run.line_top + border + padding - 3.;
                let w = bg_end - raw_x; // both raw, offset only affects position not size
                let h = run.line_height;

                pixmap.fill_rect(
                    Rect::from_xywh(x, y, w, h + 6.).unwrap(),
                    &paint,
                    Transform::identity(),
                    None,
                );
            }
        }

        buffer.draw(
            &mut self.font_system,
            &mut self.swash_cache,
            cosmic_text::Color::rgb(255, 255, 255),
            |x, y, w, h, color| {
                let x = x as f32 + border + padding;
                let y = y as f32 + border + padding;

                paint.set_color_rgba8(color.r(), color.g(), color.b(), color.a());

                pixmap.fill_rect(
                    Rect::from_xywh(x, y, w as f32, h as f32).unwrap(),
                    &paint,
                    Transform::identity(),
                    None,
                );
            },
        );
    }
}
