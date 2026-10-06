//! Raster filter effects (PhotoCraft's image filters, [`effects::pixel`]) in the renderer.
//!
//! The content (an object's art with the raster effects listed before the filter) is drawn
//! offscreen into a raster at the filter's working resolution, the raster is filtered
//! ([`effects::pixel::run_filter`]) and drawn back in place of the content:
//!
//! - **Resolution.** Filters run at the view's resolution, their distances (points) turned into
//!   pixels, so the look doesn't depend on the zoom. *Fixed* filters (pixel-grid looks: noise,
//!   the gallery's filters…) run at the document's raster effects resolution and are scaled to
//!   the view. Distances beyond a filter's pixel caps, and rasters beyond
//!   [`MAX_FILTER_PIXELS`], lower the resolution instead.
//! - **Extent.** The raster covers the content plus the filter's reach (a blur spreads beyond the
//!   art). Neighbourhood filters only need what is on screen (plus their reach); filters that move
//!   pixels across the whole object (twirl, wave…) get all of it, centred on its bounds.
//! - **Cache.** A filtered raster of the whole object is kept while the object, its effects and
//!   the zoom/rotation are unchanged, so panning doesn't run the filter again.

use std::sync::Arc;

use vectorcraft_doc::Node;
use vectorcraft_effects::{self as effects, PixelFx};
use vectorcraft_geom::{Affine, Rect};
use vello_cpu::{Pixmap, RenderContext, peniko};

use crate::ink::Ink;
use crate::{Frame, Renderer};

/// Largest filtered raster, in pixels (the resolution drops beyond it).
const MAX_FILTER_PIXELS: f64 = 8.0e6;
/// Bytes of filtered rasters the cache keeps.
const CACHE_BYTES: usize = 192 << 20;

/// A filtered raster of a whole object, positioned relative to the view-space position of
/// `anchor` (a document point), at `k` raster pixels per view pixel.
pub(crate) struct PixelEntry {
    node: Node,
    fx: Vec<effects::RasterFx>,
    linear: [u64; 4],
    k: f64,
    anchor: vectorcraft_geom::Point,
    /// View-pixel offset of the raster's top-left from the anchor's view position.
    rel: (f64, f64),
    image: Arc<Pixmap>,
    pub(crate) stamp: u64,
}

/// Cache key: (object address, effect slot, ink plane).
pub(crate) type PixelKey = (usize, usize, Ink);

/// Pixels per document unit of the document's raster effects resolution.
fn effects_scale(doc: &vectorcraft_doc::Document) -> f64 {
    let s = doc.raster_effects_ppi / 72.0;
    if s.is_finite() { s.clamp(1.0 / 72.0, 2400.0 / 72.0) } else { 1.0 }
}

/// Where and at which resolution a filter's raster is made.
struct Plan {
    /// Raster pixels per view pixel.
    k: f64,
    /// Raster origin in scaled view pixels (view pixels × `k`).
    origin: (f64, f64),
    size: (u16, u16),
    /// The content's box in raster pixels (the filter's reference bounds).
    bounds: [i32; 4],
    /// Pixels per document unit in the raster.
    scale: f64,
    /// The raster covers everything the filter writes (it can be cached and moved).
    whole: bool,
}

/// The raster for filter `fx` on content reaching `reach` (document space), drawn in frame `f`
/// on a context of `screen` (view pixels); `None` when nothing of it shows.
fn plan(f: &Frame, fx: &PixelFx, reach: Rect, screen: Rect) -> Option<Plan> {
    let view_scale = 1.0 / f.px;
    if !(view_scale.is_finite() && view_scale > 0.0) || !reach.is_finite() {
        return None;
    }
    let target = if fx.fixed { effects_scale(f.doc) } else { view_scale };
    let mut scale = target.min(fx.max_scale()).max(1e-4);
    let margin = fx.outset_at(scale);
    let content = f.view.transform_rect_bbox(reach);
    let full = f.view.transform_rect_bbox(reach.inflate(margin, margin));
    // Neighbourhood filters only need what is on screen, plus what they read around it.
    let (area, whole) = if fx.is_global() {
        (full, true)
    } else {
        let m = margin * view_scale + 2.0;
        let visible = full.intersect(screen.inflate(m, m));
        (visible, visible == full)
    };
    if !(area.width() >= 0.5 && area.height() >= 0.5) {
        return None;
    }
    let mut k = scale / view_scale;
    let n = area.width() * area.height() * k * k;
    if n > MAX_FILTER_PIXELS {
        k *= (MAX_FILTER_PIXELS / n).sqrt();
        scale = k * view_scale;
    }
    let (x0, y0) = ((area.x0 * k).floor(), (area.y0 * k).floor());
    let (w, h) = ((area.x1 * k).ceil() - x0, (area.y1 * k).ceil() - y0);
    if !(1.0..=u16::MAX as f64).contains(&w) || !(1.0..=u16::MAX as f64).contains(&h) {
        return None;
    }
    let px = |v: f64, o: f64| (v * k - o).round().clamp(-1e9, 1e9) as i32;
    let bounds = [px(content.x0, x0), px(content.y0, y0), px(content.x1, x0), px(content.y1, y0)];
    Some(Plan { k, origin: (x0, y0), size: (w as u16, h as u16), bounds, scale, whole })
}

impl Renderer {
    /// Draw what `paint` paints (document reach `reach`) through raster filter `fx` (see the
    /// module docs). `before` are the raster effects `paint` applies first; with `key`, a filtered
    /// raster of the whole object `node` is cached under it. A filter that fails leaves the
    /// content unfiltered.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn pixel_fx(
        &mut self,
        ctx: &mut RenderContext,
        f: &Frame,
        node: &Node,
        key: Option<PixelKey>,
        before: &[effects::RasterFx],
        reach: Rect,
        fx: &PixelFx,
        paint: &mut dyn FnMut(&mut Self, &mut RenderContext, &Frame),
    ) {
        let screen = Rect::new(0.0, 0.0, ctx.width() as f64, ctx.height() as f64);
        let Some(plan) = plan(f, fx, reach, screen) else { return };
        let c = f.view.as_coeffs();
        let linear = [c[0].to_bits(), c[1].to_bits(), c[2].to_bits(), c[3].to_bits()];
        let stamp = self.stamp;
        let anchor = reach.origin();
        let hit = key.and_then(|key| {
            self.pixel_cache.get_mut(&key).filter(|e| e.linear == linear && e.k == plan.k && e.node == *node && fx_matches(&e.fx, before, fx)).map(
                |e| {
                    e.stamp = stamp;
                    let p = f.view * e.anchor;
                    (e.image.clone(), e.k, (p.x * e.k + e.rel.0).round(), (p.y * e.k + e.rel.1).round())
                },
            )
        });
        let (image, k, x0, y0) = match hit {
            Some(h) => h,
            None => {
                let (x0, y0) = plan.origin;
                let (w, h) = plan.size;
                let mut off = f.offscreen_context(w, h);
                let view = Affine::translate((-x0, -y0)) * Affine::scale(plan.k) * f.view;
                let visible = view.inverse().transform_rect_bbox(Rect::new(0.0, 0.0, w as f64, h as f64));
                let fr = Frame { mt: false, view, visible, px: f.px / plan.k, ..*f };
                self.inside_layer(|r| paint(r, &mut off, &fr));
                off.flush();
                let mut pm = Pixmap::new(w, h);
                off.render(&mut pm, &mut self.resources);
                let bytes = pm.data_as_u8_slice_mut();
                let filtered = fx.filter(plan.scale).and_then(|filter| effects::pixel::run_filter(bytes, w as u32, h as u32, &filter, plan.bounds));
                if let Err(e) = filtered {
                    log::warn!("{}: {e}", fx.label());
                }
                pm.recompute_may_have_transparency();
                let image = Arc::new(pm);
                if let Some(key) = key.filter(|_| plan.whole) {
                    let p = f.view * anchor;
                    self.cache_pixels(
                        key,
                        PixelEntry {
                            node: node.clone(),
                            fx: before.iter().cloned().chain([effects::RasterFx::Pixel(fx.clone())]).collect(),
                            linear,
                            k: plan.k,
                            anchor,
                            rel: (x0 - p.x * plan.k, y0 - p.y * plan.k),
                            image: image.clone(),
                            stamp,
                        },
                    );
                }
                (image, plan.k, x0, y0)
            }
        };
        // Back to view pixels: whole pixels at the view's resolution draw exactly.
        let (w, h) = (image.width() as f64, image.height() as f64);
        let exact = (k - 1.0).abs() < 1e-9;
        ctx.set_transform(Affine::scale(1.0 / k) * Affine::translate((x0, y0)));
        let quality = if exact { peniko::ImageQuality::Low } else { peniko::ImageQuality::Medium };
        let sampler = peniko::ImageSampler { quality, ..Default::default() };
        ctx.set_paint(vello_cpu::Image { image: vello_cpu::ImageSource::Pixmap(image), sampler });
        ctx.fill_rect(&Rect::new(0.0, 0.0, w, h));
        ctx.set_transform(Affine::IDENTITY);
    }

    /// Keep `e` under `key`, dropping stale rasters (and the oldest beyond [`CACHE_BYTES`]).
    fn cache_pixels(&mut self, key: PixelKey, e: PixelEntry) {
        let size = |e: &PixelEntry| e.image.width() as usize * e.image.height() as usize * 4;
        if size(&e) > CACHE_BYTES / 2 {
            return;
        }
        let g = self.stamp;
        self.pixel_cache.retain(|_, e| g.saturating_sub(e.stamp) <= 3);
        let mut total: usize = self.pixel_cache.values().map(size).sum::<usize>() + size(&e);
        while total > CACHE_BYTES {
            let Some(oldest) = self.pixel_cache.iter().min_by_key(|(_, e)| e.stamp).map(|(k, _)| *k) else { break };
            if let Some(old) = self.pixel_cache.remove(&oldest) {
                total = total.saturating_sub(size(&old));
            }
        }
        self.pixel_cache.insert(key, e);
    }
}

/// Is a cached raster's effect list `cached` the list `before` + `fx`?
fn fx_matches(cached: &[effects::RasterFx], before: &[effects::RasterFx], fx: &PixelFx) -> bool {
    match cached.split_last() {
        Some((effects::RasterFx::Pixel(last), rest)) => last == fx && rest == before,
        _ => false,
    }
}
