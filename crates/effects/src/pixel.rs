//! Raster effects run by PhotoCraft's image filters (`photocraft-algo`): Effect › Blur (besides
//! the native Gaussian Blur), Sharpen, Noise, Pixelate, Render, Stylize, Distort, Other, Video and
//! the gallery filters (Artistic, Brush Strokes, Sketch, Texture…).
//!
//! An effect is `{id, params}` like any other; [`PIXEL_EFFECTS`] and the gallery list declare its
//! menu, parameter documentation, dialog defaults and which parameters are distances in points.
//! The renderer draws the object's art offscreen, hands the premultiplied RGBA pixels to
//! [`run_filter`] (which converts them to a straight-alpha float [`Surface`] and back) and draws
//! the result in place of the art.
//!
//! **Resolution.** Distances (`radius`, `distance`, `cellSize`…) are in points and become pixels
//! at the resolution the effect is evaluated at ([`PixelFx::filter`]), so the look doesn't change
//! with zoom or export resolution. Filters whose look is tied to a pixel grid (noise grain, the
//! gallery's brush sizes, Facet, Find Edges…) are *fixed*: they run at the document's raster
//! effects resolution whatever the zoom ([`PixelFx::fixed`]), as raster effects do when output.
//! Distances too large for the pixel caps lower the evaluation resolution
//! ([`PixelFx::max_scale`]) instead of being clipped.
//!
//! Parameters are validated: non-numbers, non-finite numbers and unknown choices are errors
//! ([`check_params`]); numbers are clamped to the dialog ranges.

use std::sync::OnceLock;

use photocraft_algo::{FilterParams, GalleryEffect, GalleryFilter, GalleryParamKind, Halo};
use photocraft_color::PixelFormat;
use photocraft_geom::Rect as PRect;
use photocraft_raster::Surface;
use serde_json::{Map, Value, json};

/// One raster-filter effect of the catalogue.
#[derive(Clone, Copy, Debug)]
pub struct PixelDef {
    pub id: &'static str,
    pub label: &'static str,
    pub menu: &'static [&'static str],
    pub params: &'static str,
    /// Dialog defaults (JSON object text).
    pub defaults: &'static str,
    /// Parameters that are distances in points.
    pub lengths: &'static [&'static str],
    /// Evaluated at the document's raster effects resolution (pixel-grid looks).
    pub fixed: bool,
}

/// The Effect-menu parent of the raster filter submenus (`["Effect", RASTER_MENUS_PARENT, …]`;
/// the blurs go in Effect › Blur with the native Gaussian Blur).
pub const RASTER_MENUS_PARENT: &str = "Raster Effects";
const RASTER: &str = RASTER_MENUS_PARENT;
const BLUR: &[&str] = &["Effect", "Blur"];
const SHARPEN: &[&str] = &["Effect", RASTER, "Sharpen"];
const NOISE: &[&str] = &["Effect", RASTER, "Noise"];
const PIXELATE: &[&str] = &["Effect", RASTER, "Pixelate"];
const RENDER: &[&str] = &["Effect", RASTER, "Render"];
const STYLIZE: &[&str] = &["Effect", RASTER, "Stylize"];
const DISTORT: &[&str] = &["Effect", RASTER, "Distort"];
const OTHER: &[&str] = &["Effect", RASTER, "Other"];
const VIDEO: &[&str] = &["Effect", RASTER, "Video"];

/// The raster effect submenus in menu order (gallery folders included).
pub const RASTER_MENUS: [&str; 13] =
    ["Artistic", "Blur", "Brush Strokes", "Distort", "Noise", "Pixelate", "Render", "Sharpen", "Sketch", "Stylize", "Texture", "Video", "Other"];

const fn d(
    id: &'static str,
    label: &'static str,
    menu: &'static [&'static str],
    params: &'static str,
    defaults: &'static str,
    lengths: &'static [&'static str],
    fixed: bool,
) -> PixelDef {
    PixelDef { id, label, menu, params, defaults, lengths, fixed }
}

/// The raster filters besides the gallery's (see [`gallery_defs`]).
pub const PIXEL_EFFECTS: &[PixelDef] = &[
    // ---- Blur ----
    d("blur.box", "Box Blur…", BLUR, "{radius: pt (0.1–500, 5)}", r#"{"radius":5.0}"#, &["radius"], false),
    d(
        "blur.motion",
        "Motion Blur…",
        BLUR,
        "{angle: ° (-360–360, 0), distance: pt (1–1000, 10)}",
        r#"{"angle":0.0,"distance":10.0}"#,
        &["distance"],
        false,
    ),
    d(
        "blur.radial",
        "Radial Blur…",
        BLUR,
        "{amount: 1–100 (10), method: \"spin\"|\"zoom\", centerX, centerY: 0–1 of the object's bounds (0.5)}",
        r#"{"amount":10.0,"method":"spin","centerX":0.5,"centerY":0.5}"#,
        &[],
        false,
    ),
    d(
        "blur.smart",
        "Smart Blur…",
        BLUR,
        "{radius: pt (0.1–100, 3), threshold: levels (0.1–100, 25), quality: \"low\"|\"medium\"|\"high\", mode: \"normal\"|\"edgeOnly\"|\"overlayEdge\"}",
        r#"{"radius":3.0,"threshold":25.0,"quality":"medium","mode":"normal"}"#,
        &["radius"],
        false,
    ),
    d(
        "blur.surface",
        "Surface Blur…",
        BLUR,
        "{radius: pt (1–100, 5), threshold: levels (2–255, 15)}",
        r#"{"radius":5.0,"threshold":15.0}"#,
        &["radius"],
        false,
    ),
    d(
        "blur.lens",
        "Lens Blur…",
        BLUR,
        "{radius: pt (0–100, 15), blades: 3–8 (6), curvature: 0–100 (0), rotation: ° (0), brightness: specular 0–100 (0), threshold: 0–255 (255), noise: 0–100 (0), distribution: \"uniform\"|\"gaussian\", monochromatic: bool, seed: int}",
        r#"{"radius":15.0,"blades":6,"curvature":0.0,"rotation":0.0,"brightness":0.0,"threshold":255.0,"noise":0.0,"distribution":"uniform","monochromatic":false,"seed":0}"#,
        &["radius"],
        false,
    ),
    d(
        "blur.shape",
        "Shape Blur…",
        BLUR,
        "{radius: pt (1–500, 10), shape: \"circle\"|\"ring\"|\"square\"|\"diamond\"|\"triangle\"|\"hexagon\"|\"star\"|\"heart\"|\"cross\"}",
        r#"{"radius":10.0,"shape":"circle"}"#,
        &["radius"],
        false,
    ),
    d(
        "blur.tiltShift",
        "Tilt-Shift…",
        BLUR,
        "{blur: pt (0–500, 15), centerX, centerY: 0–1 of the bounds (0.5), angle: ° (0), focus, transition: 0–1 of the shorter side (0.25)}",
        r#"{"blur":15.0,"centerX":0.5,"centerY":0.5,"angle":0.0,"focus":0.25,"transition":0.25}"#,
        &["blur"],
        false,
    ),
    d(
        "blur.iris",
        "Iris Blur…",
        BLUR,
        "{blur: pt (0–500, 15), centerX, centerY: 0–1 (0.5), radiusX (0.35), radiusY (0.25): 0–2 of the shorter side, angle: ° (0), roundness: 0–100 (0), feather: 0–1 (0.5)} the ellipse stays sharp",
        r#"{"blur":15.0,"centerX":0.5,"centerY":0.5,"radiusX":0.35,"radiusY":0.25,"angle":0.0,"roundness":0.0,"feather":0.5}"#,
        &["blur"],
        false,
    ),
    d(
        "blur.spin",
        "Spin Blur…",
        BLUR,
        "{blurAngle: ° (0–360, 15), centerX, centerY: 0–1 (0.5), radiusX, radiusY: 0–2 of the shorter side (0.3), angle: ° (0)}",
        r#"{"blurAngle":15.0,"centerX":0.5,"centerY":0.5,"radiusX":0.3,"radiusY":0.3,"angle":0.0}"#,
        &[],
        false,
    ),
    // ---- Sharpen ----
    d(
        "sharpen.unsharpMask",
        "Unsharp Mask…",
        SHARPEN,
        "{amount: % (1–500, 50), radius: pt (0.1–250, 1), threshold: levels (0–255, 0)}",
        r#"{"amount":50.0,"radius":1.0,"threshold":0.0}"#,
        &["radius"],
        false,
    ),
    d(
        "sharpen.smart",
        "Smart Sharpen…",
        SHARPEN,
        "{amount: % (1–500, 100), radius: pt (0.1–64, 1), reduceNoise: % (0–100, 10)}",
        r#"{"amount":100.0,"radius":1.0,"reduceNoise":10.0}"#,
        &["radius"],
        false,
    ),
    // ---- Noise ----
    d(
        "noise.add",
        "Add Noise…",
        NOISE,
        "{amount: % (0–400, 10), distribution: \"uniform\"|\"gaussian\", monochromatic: bool, seed: int} (grain: a pixel at the raster effects resolution)",
        r#"{"amount":10.0,"distribution":"uniform","monochromatic":false,"seed":0}"#,
        &[],
        true,
    ),
    d("noise.median", "Median…", NOISE, "{radius: pt (1–100, 1)}", r#"{"radius":1.0}"#, &["radius"], false),
    d(
        "noise.dustAndScratches",
        "Dust & Scratches…",
        NOISE,
        "{radius: pt (1–100, 1), threshold: levels (0–255, 0)}",
        r#"{"radius":1.0,"threshold":0.0}"#,
        &["radius"],
        false,
    ),
    d(
        "noise.reduce",
        "Reduce Noise…",
        NOISE,
        "{strength: 0–10 (6), preserveDetails: % (60), reduceColorNoise: % (45), sharpenDetails: % (25), removeJpegArtifact: bool}",
        r#"{"strength":6.0,"preserveDetails":60.0,"reduceColorNoise":45.0,"sharpenDetails":25.0,"removeJpegArtifact":false}"#,
        &[],
        true,
    ),
    // ---- Pixelate ----
    d(
        "pixelate.colorHalftone",
        "Color Halftone…",
        PIXELATE,
        "{maxRadius: pt (1–127, 8), channel1..channel4: screen angles ° (108, 162, 90, 45)}",
        r#"{"maxRadius":8.0,"channel1":108.0,"channel2":162.0,"channel3":90.0,"channel4":45.0}"#,
        &["maxRadius"],
        false,
    ),
    d(
        "pixelate.crystallize",
        "Crystallize…",
        PIXELATE,
        "{cellSize: pt (3–300, 10), seed: int}",
        r#"{"cellSize":10.0,"seed":0}"#,
        &["cellSize"],
        false,
    ),
    d("pixelate.facet", "Facet", PIXELATE, "{}", "{}", &[], true),
    d("pixelate.fragment", "Fragment", PIXELATE, "{}", "{}", &[], true),
    d(
        "pixelate.mezzotint",
        "Mezzotint…",
        PIXELATE,
        "{type: \"fineDots\"|\"mediumDots\"|\"grainyDots\"|\"coarseDots\"|\"shortLines\"|\"mediumLines\"|\"longLines\"|\"shortStrokes\"|\"mediumStrokes\"|\"longStrokes\", seed: int}",
        r#"{"type":"fineDots","seed":0}"#,
        &[],
        true,
    ),
    d("pixelate.mosaic", "Mosaic…", PIXELATE, "{cellSize: pt (1–200, 8)}", r#"{"cellSize":8.0}"#, &["cellSize"], false),
    d(
        "pixelate.pointillize",
        "Pointillize…",
        PIXELATE,
        "{cellSize: pt (3–300, 5), background: \"#rrggbb\"|\"none\" (between the dots; none), seed: int}",
        r##"{"cellSize":5.0,"background":"none","seed":0}"##,
        &["cellSize"],
        false,
    ),
    // ---- Render ----
    d(
        "render.lensFlare",
        "Lens Flare…",
        RENDER,
        "{brightness: % (10–300, 100), centerX, centerY: 0–1 of the bounds (0.3), lens: \"zoom\"|\"prime35\"|\"prime105\"|\"moviePrime\"}",
        r#"{"brightness":100.0,"centerX":0.3,"centerY":0.3,"lens":"zoom"}"#,
        &[],
        false,
    ),
    d(
        "render.lightingEffects",
        "Lighting Effects…",
        RENDER,
        "{light: \"spot\"|\"point\"|\"infinite\", intensity: -100–100 (75), color: \"#rrggbb\" (white), x, y: light position 0–1 of the bounds (0.25, 0.2), angle: ° (infinite: 135), gloss, metallic, exposure: -100–100 (0), ambience: -100–100 (8), texture: \"none\"|\"red\"|\"green\"|\"blue\"|\"alpha\"|\"luminance\" (bump channel), height: 0–100 (50)}",
        r##"{"light":"spot","intensity":75.0,"color":"#ffffff","x":0.25,"y":0.2,"angle":135.0,"gloss":0.0,"metallic":0.0,"exposure":0.0,"ambience":8.0,"texture":"none","height":50.0}"##,
        &[],
        true,
    ),
    // ---- Stylize ----
    d(
        "stylize.diffuse",
        "Diffuse…",
        STYLIZE,
        "{mode: \"normal\"|\"darkenOnly\"|\"lightenOnly\"|\"anisotropic\", seed: int}",
        r#"{"mode":"normal","seed":0}"#,
        &[],
        true,
    ),
    d(
        "stylize.emboss",
        "Emboss…",
        STYLIZE,
        "{angle: ° (135), height: pt (1–100, 3), amount: % (1–500, 100)}",
        r#"{"angle":135.0,"height":3.0,"amount":100.0}"#,
        &["height"],
        false,
    ),
    d(
        "stylize.extrude",
        "Extrude…",
        STYLIZE,
        "{type: \"blocks\"|\"pyramids\", size: pt (2–255, 30), depth: 1–255 (30), levelBased: bool, solidFront: bool, maskIncomplete: bool, seed: int}",
        r#"{"type":"blocks","size":30.0,"depth":30.0,"levelBased":false,"solidFront":false,"maskIncomplete":false,"seed":0}"#,
        &["size"],
        false,
    ),
    d("stylize.findEdges", "Find Edges", STYLIZE, "{}", "{}", &[], true),
    d(
        "stylize.oilPaint",
        "Oil Paint…",
        STYLIZE,
        "{stylization: 0.1–10 (2), cleanliness: 0–10 (7), scale: 0.1–10 (1), bristleDetail: 0–10 (4), lighting: bool (true), angle: ° (-60), shine: 0–10 (1)}",
        r#"{"stylization":2.0,"cleanliness":7.0,"scale":1.0,"bristleDetail":4.0,"lighting":true,"angle":-60.0,"shine":1.0}"#,
        &[],
        true,
    ),
    d("stylize.solarize", "Solarize", STYLIZE, "{}", "{}", &[], false),
    d(
        "stylize.tiles",
        "Tiles…",
        STYLIZE,
        "{count: tiles along the shorter side 1–99 (10), maxOffset: % (1–99, 10), fill: \"background\"|\"foreground\"|\"inverse\"|\"unaltered\", foreground: \"#rrggbb\" (black), background: \"#rrggbb\"|\"none\" (none), seed: int}",
        r##"{"count":10,"maxOffset":10.0,"fill":"background","foreground":"#000000","background":"none","seed":0}"##,
        &[],
        false,
    ),
    d(
        "stylize.traceContour",
        "Trace Contour…",
        STYLIZE,
        "{level: 0–255 (128), upper: bool (true: trace above the level)}",
        r#"{"level":128.0,"upper":true}"#,
        &[],
        true,
    ),
    d(
        "stylize.wind",
        "Wind…",
        STYLIZE,
        "{method: \"wind\"|\"blast\"|\"stagger\", fromRight: bool, seed: int}",
        r#"{"method":"wind","fromRight":false,"seed":0}"#,
        &[],
        true,
    ),
    // ---- Distort ----
    d("distort.twirl", "Twirl…", DISTORT, "{angle: ° (-999–999, 50)}", r#"{"angle":50.0}"#, &[], false),
    d("distort.pinch", "Pinch…", DISTORT, "{amount: % (-100–100, 50)}", r#"{"amount":50.0}"#, &[], false),
    d(
        "distort.spherize",
        "Spherize…",
        DISTORT,
        "{amount: % (-100–100, 100), mode: \"normal\"|\"horizontalOnly\"|\"verticalOnly\"}",
        r#"{"amount":100.0,"mode":"normal"}"#,
        &[],
        false,
    ),
    d(
        "distort.wave",
        "Wave…",
        DISTORT,
        "{generators: 1–50 (5), wavelengthMin, wavelengthMax: pt (1–999; 10, 120), amplitudeMin, amplitudeMax: pt (1–999; 5, 35), type: \"sine\"|\"triangle\"|\"square\", undefined: \"wrap\"|\"repeat\"|\"transparent\", seed: int}",
        r#"{"generators":5,"wavelengthMin":10.0,"wavelengthMax":120.0,"amplitudeMin":5.0,"amplitudeMax":35.0,"type":"sine","undefined":"transparent","seed":0}"#,
        &["wavelengthMin", "wavelengthMax", "amplitudeMin", "amplitudeMax"],
        false,
    ),
    d(
        "distort.ripple",
        "Ripple…",
        DISTORT,
        "{amount: % (-999–999, 100), size: \"small\"|\"medium\"|\"large\"}",
        r#"{"amount":100.0,"size":"medium"}"#,
        &[],
        true,
    ),
    d(
        "distort.polarCoordinates",
        "Polar Coordinates…",
        DISTORT,
        "{mode: \"rectangularToPolar\"|\"polarToRectangular\"}",
        r#"{"mode":"rectangularToPolar"}"#,
        &[],
        false,
    ),
    d(
        "distort.shear",
        "Shear…",
        DISTORT,
        "{points: \"t,offset t,offset …\" curve points top (t 0) to bottom (t 1), offset -1–1 of half the width (\"0,0 0.5,0.4 1,0\"), undefined: \"wrap\"|\"repeat\"|\"transparent\"}",
        r#"{"points":"0,0 0.5,0.4 1,0","undefined":"transparent"}"#,
        &[],
        false,
    ),
    d(
        "distort.rasterZigZag",
        "ZigZag…",
        DISTORT,
        "{amount: -100–100 (10), ridges: 0–20 (5), style: \"aroundCenter\"|\"outFromCenter\"|\"pondRipples\"} (the raster filter; distort.zigZag is the path effect)",
        r#"{"amount":10.0,"ridges":5.0,"style":"pondRipples"}"#,
        &[],
        false,
    ),
    // ---- Other ----
    d("other.highPass", "High Pass…", OTHER, "{radius: pt (0.1–250, 10)}", r#"{"radius":10.0}"#, &["radius"], false),
    d(
        "other.minimum",
        "Minimum…",
        OTHER,
        "{radius: pt (0.1–100, 1), preserve: \"squareness\"|\"roundness\"}",
        r#"{"radius":1.0,"preserve":"squareness"}"#,
        &["radius"],
        false,
    ),
    d(
        "other.maximum",
        "Maximum…",
        OTHER,
        "{radius: pt (0.1–100, 1), preserve: \"squareness\"|\"roundness\"}",
        r#"{"radius":1.0,"preserve":"squareness"}"#,
        &["radius"],
        false,
    ),
    d(
        "other.offset",
        "Offset…",
        OTHER,
        "{horizontal, vertical: pt (-5000–5000, 0), undefined: \"wrap\"|\"repeat\"|\"transparent\"} within the object's bounds",
        r#"{"horizontal":0.0,"vertical":0.0,"undefined":"wrap"}"#,
        &["horizontal", "vertical"],
        false,
    ),
    d(
        "other.custom",
        "Custom…",
        OTHER,
        "{kernel: 25 numbers -999–999, row by row (a 5×5 kernel, centre = 13th), scale: 1–9999 (1), offset: -9999–9999 (0)}",
        r#"{"kernel":"0 0 0 0 0 0 0 -1 0 0 0 -1 5 -1 0 0 0 -1 0 0 0 0 0 0 0","scale":1.0,"offset":0.0}"#,
        &[],
        true,
    ),
    d("other.hsbHsl", "HSB/HSL…", OTHER, "{input, output: \"rgb\"|\"hsb\"|\"hsl\" (rgb → hsb)}", r#"{"input":"rgb","output":"hsb"}"#, &[], false),
    d("other.invert", "Invert", OTHER, "{}", "{}", &[], false),
    d("other.desaturate", "Desaturate", OTHER, "{}", "{}", &[], false),
    // ---- Video ----
    d(
        "video.deInterlace",
        "De-Interlace…",
        VIDEO,
        "{eliminate: \"odd\"|\"even\" (lines), createBy: \"interpolation\"|\"duplication\"}",
        r#"{"eliminate":"odd","createBy":"interpolation"}"#,
        &[],
        true,
    ),
    d("video.ntscColors", "NTSC Colors", VIDEO, "{}", "{}", &[], false),
];

/// Gallery filter colour parameters: (key in the filter's notation, effect param, default).
const GALLERY_COLOURS: [(&str, &str, &str); 3] =
    [("foreground", "foreground", "#000000"), ("background", "background", "#ffffff"), ("glowColor", "glowColor", "#009eff")];

/// The gallery filters (`gallery.<key>`), in the gallery's order.
pub fn gallery_defs() -> &'static [PixelDef] {
    static DEFS: OnceLock<Vec<PixelDef>> = OnceLock::new();
    DEFS.get_or_init(|| {
        use vectorcraft_plugins::registry::intern;
        GalleryFilter::ALL
            .iter()
            .map(|f| {
                let mut defaults = Map::new();
                let mut doc = vec![];
                for p in f.params() {
                    let (v, text) = match &p.kind {
                        GalleryParamKind::Range { min, max, default } => (json!(*default as f64), format!("{}: {min}–{max} ({default})", p.key)),
                        GalleryParamKind::Choice(c) => {
                            let names: Vec<String> = c.iter().map(|n| format!("\"{n}\"")).collect();
                            (json!(c.first().copied().unwrap_or_default()), format!("{}: {}", p.key, names.join("|")))
                        }
                        GalleryParamKind::Bool(b) => (json!(*b), format!("{}: bool", p.key)),
                    };
                    defaults.insert(p.key.to_string(), v);
                    doc.push(text);
                }
                for (key, param, default) in GALLERY_COLOURS {
                    if f.params_doc().contains(&format!("\"{key}\":json")) {
                        defaults.insert(param.to_string(), json!(default));
                        doc.push(format!("{param}: \"#rrggbb\" ({default})"));
                    }
                }
                let menu: &'static [&'static str] = match f.category() {
                    "Artistic" => &["Effect", RASTER, "Artistic"],
                    "Brush Strokes" => &["Effect", RASTER, "Brush Strokes"],
                    "Distort" => DISTORT,
                    "Sketch" => &["Effect", RASTER, "Sketch"],
                    "Stylize" => STYLIZE,
                    _ => &["Effect", RASTER, "Texture"],
                };
                PixelDef {
                    id: intern(&format!("gallery.{}", f.key())),
                    label: intern(&format!("{}…", f.name())),
                    menu,
                    params: intern(&format!("{{{}}} (sizes in pixels at the document raster effects resolution)", doc.join(", "))),
                    defaults: intern(&Value::Object(defaults).to_string()),
                    lengths: &[],
                    fixed: true,
                }
            })
            .collect()
    })
}

/// Every raster filter effect: [`PIXEL_EFFECTS`] then the gallery's.
pub fn pixel_defs() -> impl Iterator<Item = &'static PixelDef> {
    PIXEL_EFFECTS.iter().chain(gallery_defs())
}

/// The definition of raster filter effect `id`.
pub fn pixel_def(id: &str) -> Option<&'static PixelDef> {
    if let Some(key) = id.strip_prefix("gallery.") {
        return gallery_defs().iter().find(|d| d.id.strip_prefix("gallery.") == Some(key));
    }
    PIXEL_EFFECTS.iter().find(|d| d.id == id)
}

/// Is `id` a raster filter effect (run by PhotoCraft's filters)?
pub fn is_pixel(id: &str) -> bool {
    pixel_def(id).is_some()
}

/// A raster filter effect with its parameters (over the defaults), ready to run.
#[derive(Clone, Debug, PartialEq)]
pub struct PixelFx {
    pub id: &'static str,
    /// Parameters over the dialog defaults.
    pub params: Value,
    /// Evaluated at the document's raster effects resolution rather than the view's.
    pub fixed: bool,
}

/// Largest blur-like reach in pixels: a distance beyond it lowers the evaluation resolution.
const MAX_REACH_PX: f64 = 250.0;
/// Largest neighbourhood radius in pixels of the filters whose cost grows with its square.
const MAX_SQUARE_PX: f64 = 60.0;

impl PixelFx {
    /// The effect `id` with `params` (merged over its defaults); `None` for other effects.
    pub fn new(id: &str, params: &Value) -> Option<Self> {
        let def = pixel_def(id)?;
        let mut merged = match serde_json::from_str::<Value>(def.defaults) {
            Ok(Value::Object(m)) => m,
            _ => Map::new(),
        };
        if let Value::Object(p) = params {
            for (k, v) in p {
                merged.insert(k.clone(), v.clone());
            }
        }
        Some(Self { id: def.id, params: Value::Object(merged), fixed: def.fixed })
    }

    /// Menu label without the ellipsis.
    pub fn label(&self) -> &'static str {
        pixel_def(self.id).map(|d| d.label.trim_end_matches('…')).unwrap_or(self.id)
    }

    /// Pixel caps of the distance parameters: (key, largest pixels).
    fn caps(&self) -> &'static [(&'static str, f64)] {
        match self.id {
            "blur.surface" | "noise.median" | "noise.dustAndScratches" | "blur.shape" => &[("radius", MAX_SQUARE_PX)],
            "other.minimum" | "other.maximum" => &[("radius", 100.0)],
            "blur.smart" | "blur.lens" => &[("radius", 100.0)],
            "sharpen.smart" => &[("radius", 64.0)],
            "pixelate.colorHalftone" => &[("maxRadius", 127.0)],
            "pixelate.crystallize" | "pixelate.pointillize" => &[("cellSize", 300.0)],
            "pixelate.mosaic" => &[("cellSize", 400.0)],
            "stylize.emboss" => &[("height", 100.0)],
            "stylize.extrude" => &[("size", 255.0)],
            "blur.motion" => &[("distance", 2.0 * MAX_REACH_PX)],
            "blur.tiltShift" | "blur.iris" => &[("blur", MAX_REACH_PX)],
            "distort.wave" => &[("wavelengthMax", 999.0), ("amplitudeMax", 999.0), ("wavelengthMin", 999.0), ("amplitudeMin", 999.0)],
            "other.offset" => &[("horizontal", 5000.0), ("vertical", 5000.0)],
            _ => &[("radius", MAX_REACH_PX)],
        }
    }

    /// The highest resolution (pixels per point) this effect can run at with its distances
    /// within their pixel caps.
    pub fn max_scale(&self) -> f64 {
        let Some(def) = pixel_def(self.id) else { return f64::INFINITY };
        let mut s = f64::INFINITY;
        for key in def.lengths {
            let Some(cap) = self.caps().iter().find(|(k, _)| k == key).map(|c| c.1) else { continue };
            let v = crate::util::num(&self.params, key, 0.0).abs();
            if v > 1e-9 {
                s = s.min(cap / v);
            }
        }
        s
    }

    /// The PhotoCraft filter at `scale` pixels per point (distances become pixels).
    pub fn filter(&self, scale: f64) -> Result<FilterParams, String> {
        if !(scale.is_finite() && scale > 0.0) {
            return Err(format!("{}: invalid resolution {scale}", self.id));
        }
        build(self.id, &P { p: &self.params, id: self.id, scale })
    }

    /// Whether the filter moves pixels around the whole object (twirl, wave…) rather than
    /// reading a neighbourhood.
    pub fn is_global(&self) -> bool {
        self.filter(1.0).is_ok_and(|f| f.is_global())
    }

    /// How far the filter writes beyond the art at `scale` pixels per point, in points (distortions
    /// stay within the object's bounds).
    pub fn outset_at(&self, scale: f64) -> f64 {
        match self.filter(scale).map(|f| f.halo()) {
            Ok(Halo::Radius(r)) => (r.max(0) as f64 / scale).min(MAX_OUTSET),
            _ => 0.0,
        }
    }

    /// [`Self::outset_at`] at 72 ppi (one pixel per point), an upper bound for the documents'
    /// raster effects resolutions.
    pub fn outset(&self) -> f64 {
        self.outset_at(1.0)
    }
}

/// Largest outset of one raster filter, in points.
const MAX_OUTSET: f64 = 2000.0;

/// Check effect `id`'s parameters (over its defaults): `Err` names the bad one. Other effects
/// always pass.
pub fn check_params(id: &str, params: &Value) -> Result<(), String> {
    match PixelFx::new(id, params) {
        Some(fx) => fx.filter(1.0).map(|_| ()),
        None => Ok(()),
    }
}

/// Parameter reader for [`build`].
struct P<'a> {
    p: &'a Value,
    id: &'a str,
    scale: f64,
}

impl P<'_> {
    fn err(&self, key: &str, what: &str) -> String {
        format!("{}: `{key}` must be {what}", self.id)
    }

    /// Number `key` clamped to `lo..=hi`.
    fn num(&self, key: &str, lo: f64, hi: f64) -> Result<f32, String> {
        let v = match self.p.get(key) {
            Some(Value::Number(n)) => n.as_f64(),
            Some(Value::String(s)) => {
                let t: String = s.trim().chars().take_while(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+' | 'e' | 'E')).collect();
                t.parse::<f64>().ok()
            }
            Some(Value::Bool(b)) => Some(f64::from(u8::from(*b))),
            _ => None,
        };
        match v.filter(|v| v.is_finite()) {
            Some(v) => Ok(v.clamp(lo, hi) as f32),
            None => Err(self.err(key, "a finite number")),
        }
    }

    /// Distance `key` in points (clamped to `lo..=hi` pt) as pixels, at most `cap` pixels.
    fn len(&self, key: &str, lo: f64, hi: f64, cap: f64) -> Result<f32, String> {
        let pt = self.num(key, lo, hi)? as f64;
        Ok((pt * self.scale).clamp(-cap, cap) as f32)
    }

    fn int(&self, key: &str, lo: f64, hi: f64) -> Result<u32, String> {
        Ok(self.num(key, lo, hi)?.round().max(0.0) as u32)
    }

    fn seed(&self) -> Result<u32, String> {
        match self.p.get("seed") {
            None | Some(Value::Null) => Ok(0),
            Some(_) => self.int("seed", 0.0, u32::MAX as f64),
        }
    }

    fn flag(&self, key: &str) -> Result<bool, String> {
        match self.p.get(key) {
            Some(Value::Bool(b)) => Ok(*b),
            Some(Value::Number(n)) => Ok(n.as_f64().is_some_and(|v| v != 0.0)),
            _ => Err(self.err(key, "true or false")),
        }
    }

    /// Choice `key`, one of `options` (case-insensitive) → its canonical name.
    fn choice(&self, key: &str, options: &[&'static str]) -> Result<&'static str, String> {
        let s = self.p.get(key).and_then(Value::as_str).ok_or_else(|| self.err(key, &format!("one of {}", options.join("|"))))?;
        let norm = |s: &str| s.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
        options.iter().copied().find(|o| norm(o) == norm(s)).ok_or_else(|| self.err(key, &format!("one of {}", options.join("|"))))
    }

    /// Choice `key` deserialized as PhotoCraft's enum `T` (whose serde names are `options`).
    fn pick<T: serde::de::DeserializeOwned>(&self, key: &str, options: &[&'static str]) -> Result<T, String> {
        let name = self.choice(key, options)?;
        serde_json::from_value(json!(name)).map_err(|e| format!("{}: `{key}`: {e}", self.id))
    }

    /// Colour `key`: `"#rrggbb"`, `"#rrggbbaa"`, `"none"` or `[r, g, b(, a)]` 0–1 → straight RGBA.
    fn color(&self, key: &str) -> Result<[f32; 4], String> {
        let bad = || self.err(key, "a colour (\"#rrggbb\", \"none\" or [r, g, b])");
        match self.p.get(key) {
            Some(Value::String(s)) => {
                let s = s.trim();
                if s.eq_ignore_ascii_case("none") || s.eq_ignore_ascii_case("transparent") {
                    return Ok([0.0; 4]);
                }
                let hex = s.strip_prefix('#').unwrap_or(s);
                if !(hex.len() == 6 || hex.len() == 8) || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
                    return Err(bad());
                }
                let byte = |i: usize| hex.get(i..i + 2).and_then(|b| u8::from_str_radix(b, 16).ok()).map(|b| b as f32 / 255.0);
                let a = if hex.len() == 8 { byte(6).ok_or_else(bad)? } else { 1.0 };
                Ok([byte(0).ok_or_else(bad)?, byte(2).ok_or_else(bad)?, byte(4).ok_or_else(bad)?, a])
            }
            Some(Value::Array(a)) if (3..=4).contains(&a.len()) => {
                let mut out = [0.0, 0.0, 0.0, 1.0];
                for (o, v) in out.iter_mut().zip(a) {
                    *o = v.as_f64().filter(|v| v.is_finite()).ok_or_else(bad)?.clamp(0.0, 1.0) as f32;
                }
                Ok(out)
            }
            _ => Err(bad()),
        }
    }

    /// `count` numbers from the string (or array) `key`.
    fn numbers(&self, key: &str, lo: f64, hi: f64) -> Result<Vec<f32>, String> {
        let bad = || self.err(key, "a list of numbers");
        let vals: Vec<f64> = match self.p.get(key) {
            Some(Value::String(s)) => s
                .split(|c: char| c.is_whitespace() || c == ',' || c == ';')
                .filter(|t| !t.is_empty())
                .map(|t| t.parse::<f64>().map_err(|_| bad()))
                .collect::<Result<_, _>>()?,
            Some(Value::Array(a)) => a.iter().map(|v| v.as_f64().ok_or_else(bad)).collect::<Result<_, _>>()?,
            _ => return Err(bad()),
        };
        if vals.len() > 1024 || vals.iter().any(|v| !v.is_finite()) {
            return Err(bad());
        }
        Ok(vals.into_iter().map(|v| v.clamp(lo, hi) as f32).collect())
    }
}

const UNDEFINED: &[&str] = &["wrap", "repeat", "transparent"];
const DISTRIBUTION: &[&str] = &["uniform", "gaussian"];
const PRESERVE: &[&str] = &["squareness", "roundness"];

/// The filter for effect `id` (see [`PixelFx::filter`]).
fn build(id: &str, p: &P) -> Result<FilterParams, String> {
    use photocraft_algo as a;
    let cap = MAX_REACH_PX * 4.0;
    Ok(match id {
        "blur.box" => FilterParams::BoxBlur { radius: p.len("radius", 0.1, 500.0, cap)?.max(0.05) },
        "blur.motion" => {
            FilterParams::MotionBlur { angle: p.num("angle", -360.0, 360.0)?, distance: p.len("distance", 1.0, 1000.0, cap * 2.0)?.max(0.5) }
        }
        "blur.radial" => FilterParams::RadialBlur {
            amount: p.num("amount", 1.0, 100.0)?,
            method: p.pick("method", &["spin", "zoom"])?,
            center_x: p.num("centerX", 0.0, 1.0)?,
            center_y: p.num("centerY", 0.0, 1.0)?,
        },
        "blur.smart" => FilterParams::SmartBlur {
            radius: p.len("radius", 0.1, 100.0, 100.0)?.max(0.1),
            threshold: p.num("threshold", 0.1, 100.0)?,
            quality: p.pick("quality", &["low", "medium", "high"])?,
            mode: p.pick("mode", &["normal", "edgeOnly", "overlayEdge"])?,
        },
        "blur.surface" => {
            FilterParams::SurfaceBlur { radius: p.len("radius", 1.0, 100.0, MAX_SQUARE_PX)?.max(0.5), threshold: p.num("threshold", 2.0, 255.0)? }
        }
        "blur.lens" => FilterParams::LensBlur {
            radius: p.len("radius", 0.0, 100.0, 100.0)?,
            blades: p.int("blades", 3.0, 8.0)?,
            curvature: p.num("curvature", 0.0, 100.0)?,
            rotation: p.num("rotation", -360.0, 360.0)?,
            depth: a::DepthSource::None,
            focal_distance: 0.0,
            invert_depth: false,
            brightness: p.num("brightness", 0.0, 100.0)?,
            threshold: p.num("threshold", 0.0, 255.0)?,
            noise: p.num("noise", 0.0, 100.0)?,
            distribution: p.pick("distribution", DISTRIBUTION)?,
            monochromatic: p.flag("monochromatic")?,
            seed: p.seed()?,
            depth_map: None,
        },
        "blur.shape" => FilterParams::ShapeBlur {
            radius: p.len("radius", 1.0, 500.0, MAX_SQUARE_PX)?.max(0.5),
            shape: p.pick("shape", &["circle", "ring", "square", "diamond", "triangle", "hexagon", "star", "heart", "cross"])?,
        },
        "blur.tiltShift" => FilterParams::TiltShift {
            blur: p.len("blur", 0.0, 500.0, MAX_REACH_PX)?,
            center_x: p.num("centerX", 0.0, 1.0)?,
            center_y: p.num("centerY", 0.0, 1.0)?,
            angle: p.num("angle", -360.0, 360.0)?,
            focus: p.num("focus", 0.0, 1.0)?,
            transition: p.num("transition", 0.0, 1.0)?,
        },
        "blur.iris" => FilterParams::IrisBlur {
            pins: vec![a::IrisPin {
                x: p.num("centerX", 0.0, 1.0)?,
                y: p.num("centerY", 0.0, 1.0)?,
                radius_x: p.num("radiusX", 0.01, 2.0)?,
                radius_y: p.num("radiusY", 0.01, 2.0)?,
                angle: p.num("angle", -360.0, 360.0)?,
                roundness: p.num("roundness", 0.0, 100.0)?,
                feather: p.num("feather", 0.0, 1.0)?,
                blur: p.len("blur", 0.0, 500.0, MAX_REACH_PX)?,
            }],
        },
        "blur.spin" => FilterParams::SpinBlur {
            pins: vec![a::SpinPin {
                x: p.num("centerX", 0.0, 1.0)?,
                y: p.num("centerY", 0.0, 1.0)?,
                radius_x: p.num("radiusX", 0.01, 2.0)?,
                radius_y: p.num("radiusY", 0.01, 2.0)?,
                angle: p.num("angle", -360.0, 360.0)?,
                blur_angle: p.num("blurAngle", 0.0, 360.0)?,
            }],
        },
        "sharpen.unsharpMask" => FilterParams::UnsharpMask {
            amount: p.num("amount", 1.0, 500.0)?,
            radius: p.len("radius", 0.1, 250.0, MAX_REACH_PX)?.max(0.1),
            threshold: p.num("threshold", 0.0, 255.0)?,
        },
        "sharpen.smart" => FilterParams::SmartSharpen {
            amount: p.num("amount", 1.0, 500.0)?,
            radius: p.len("radius", 0.1, 64.0, 64.0)?.max(0.1),
            reduce_noise: p.num("reduceNoise", 0.0, 100.0)?,
        },
        "noise.add" => FilterParams::AddNoise {
            amount: p.num("amount", 0.0, 400.0)?,
            distribution: p.pick("distribution", DISTRIBUTION)?,
            monochromatic: p.flag("monochromatic")?,
            seed: p.seed()?,
        },
        "noise.median" => FilterParams::Median { radius: p.len("radius", 1.0, 100.0, MAX_SQUARE_PX)?.max(0.5) },
        "noise.dustAndScratches" => FilterParams::DustAndScratches {
            radius: p.len("radius", 1.0, 100.0, MAX_SQUARE_PX)?.max(0.5),
            threshold: p.num("threshold", 0.0, 255.0)?,
        },
        "noise.reduce" => FilterParams::ReduceNoise {
            strength: p.num("strength", 0.0, 10.0)?,
            preserve_details: p.num("preserveDetails", 0.0, 100.0)?,
            reduce_color_noise: p.num("reduceColorNoise", 0.0, 100.0)?,
            sharpen_details: p.num("sharpenDetails", 0.0, 100.0)?,
            remove_jpeg_artifact: p.flag("removeJpegArtifact")?,
        },
        "pixelate.colorHalftone" => FilterParams::ColorHalftone {
            max_radius: p.len("maxRadius", 1.0, 127.0, 127.0)?.max(1.0),
            angles: [
                p.num("channel1", -360.0, 360.0)?,
                p.num("channel2", -360.0, 360.0)?,
                p.num("channel3", -360.0, 360.0)?,
                p.num("channel4", -360.0, 360.0)?,
            ],
        },
        "pixelate.crystallize" => FilterParams::Crystallize { cell_size: p.len("cellSize", 3.0, 300.0, 300.0)?.max(1.0), seed: p.seed()? },
        "pixelate.facet" => FilterParams::Facet,
        "pixelate.fragment" => FilterParams::Fragment,
        "pixelate.mezzotint" => FilterParams::Mezzotint {
            kind: p.pick(
                "type",
                &[
                    "fineDots",
                    "mediumDots",
                    "grainyDots",
                    "coarseDots",
                    "shortLines",
                    "mediumLines",
                    "longLines",
                    "shortStrokes",
                    "mediumStrokes",
                    "longStrokes",
                ],
            )?,
            seed: p.seed()?,
        },
        "pixelate.mosaic" => FilterParams::Mosaic { cell_size: p.len("cellSize", 1.0, 200.0, 400.0)?.max(1.0) },
        "pixelate.pointillize" => FilterParams::Pointillize {
            cell_size: p.len("cellSize", 3.0, 300.0, 300.0)?.max(1.0),
            seed: p.seed()?,
            background: p.color("background")?,
        },
        "render.lensFlare" => FilterParams::LensFlare {
            brightness: p.num("brightness", 10.0, 300.0)?,
            center_x: p.num("centerX", 0.0, 1.0)?,
            center_y: p.num("centerY", 0.0, 1.0)?,
            lens: p.pick("lens", &["zoom", "prime35", "prime105", "moviePrime"])?,
        },
        "render.lightingEffects" => {
            let c = p.color("color")?;
            // The light's colour is linear RGB.
            let lin = |v: f32| if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
            let light = a::Light {
                kind: p.pick("light", &["spot", "point", "infinite"])?,
                x: p.num("x", -1.0, 2.0)?,
                y: p.num("y", -1.0, 2.0)?,
                angle: p.num("angle", -360.0, 360.0)?,
                color: [lin(c[0]), lin(c[1]), lin(c[2])],
                intensity: p.num("intensity", -100.0, 100.0)?,
                ..Default::default()
            };
            FilterParams::LightingEffects {
                lights: vec![light],
                gloss: p.num("gloss", -100.0, 100.0)?,
                metallic: p.num("metallic", -100.0, 100.0)?,
                exposure: p.num("exposure", -100.0, 100.0)?,
                ambience: p.num("ambience", -100.0, 100.0)?,
                texture: p.pick("texture", &["none", "red", "green", "blue", "alpha", "luminance"])?,
                height: p.num("height", 0.0, 100.0)?,
                white_is_high: true,
            }
        }
        "stylize.diffuse" => {
            FilterParams::Diffuse { mode: p.pick("mode", &["normal", "darkenOnly", "lightenOnly", "anisotropic"])?, seed: p.seed()? }
        }
        "stylize.emboss" => FilterParams::Emboss {
            angle: p.num("angle", -360.0, 360.0)?,
            height: p.len("height", 1.0, 100.0, 100.0)?.max(1.0),
            amount: p.num("amount", 1.0, 500.0)?,
        },
        "stylize.extrude" => FilterParams::Extrude {
            kind: p.pick("type", &["blocks", "pyramids"])?,
            size: p.len("size", 2.0, 255.0, 255.0)?.max(2.0),
            depth: p.num("depth", 1.0, 255.0)?,
            level_based: p.flag("levelBased")?,
            solid_front: p.flag("solidFront")?,
            mask_incomplete: p.flag("maskIncomplete")?,
            seed: p.seed()?,
        },
        "stylize.findEdges" => FilterParams::FindEdges,
        "stylize.oilPaint" => FilterParams::OilPaint {
            stylization: p.num("stylization", 0.1, 10.0)?,
            cleanliness: p.num("cleanliness", 0.0, 10.0)?,
            scale: p.num("scale", 0.1, 10.0)?,
            bristle_detail: p.num("bristleDetail", 0.0, 10.0)?,
            lighting: p.flag("lighting")?,
            angle: p.num("angle", -360.0, 360.0)?,
            shine: p.num("shine", 0.0, 10.0)?,
        },
        "stylize.solarize" => FilterParams::Solarize,
        "stylize.tiles" => FilterParams::Tiles {
            count: p.int("count", 1.0, 99.0)?.max(1),
            max_offset: p.num("maxOffset", 1.0, 99.0)?,
            fill: p.pick("fill", &["background", "foreground", "inverse", "unaltered"])?,
            foreground: p.color("foreground")?,
            background: p.color("background")?,
            seed: p.seed()?,
        },
        "stylize.traceContour" => FilterParams::TraceContour { level: p.num("level", 0.0, 255.0)?, upper: p.flag("upper")? },
        "stylize.wind" => {
            FilterParams::Wind { method: p.pick("method", &["wind", "blast", "stagger"])?, from_right: p.flag("fromRight")?, seed: p.seed()? }
        }
        "distort.twirl" => FilterParams::Twirl { angle: p.num("angle", -999.0, 999.0)? },
        "distort.pinch" => FilterParams::Pinch { amount: p.num("amount", -100.0, 100.0)? },
        "distort.spherize" => {
            FilterParams::Spherize { amount: p.num("amount", -100.0, 100.0)?, mode: p.pick("mode", &["normal", "horizontalOnly", "verticalOnly"])? }
        }
        "distort.wave" => {
            let (w0, w1) = (p.len("wavelengthMin", 1.0, 999.0, 999.0)?.max(1.0), p.len("wavelengthMax", 1.0, 999.0, 999.0)?.max(1.0));
            let (a0, a1) = (p.len("amplitudeMin", 1.0, 999.0, 999.0)?.max(0.0), p.len("amplitudeMax", 1.0, 999.0, 999.0)?.max(0.0));
            FilterParams::Wave {
                generators: p.int("generators", 1.0, 50.0)?.max(1),
                wavelength_min: w0.min(w1),
                wavelength_max: w0.max(w1),
                amplitude_min: a0.min(a1),
                amplitude_max: a0.max(a1),
                wave_type: p.pick("type", &["sine", "triangle", "square"])?,
                undefined: p.pick("undefined", UNDEFINED)?,
                seed: p.seed()?,
            }
        }
        "distort.ripple" => FilterParams::Ripple { amount: p.num("amount", -999.0, 999.0)?, size: p.pick("size", &["small", "medium", "large"])? },
        "distort.polarCoordinates" => FilterParams::PolarCoordinates { mode: p.pick("mode", &["rectangularToPolar", "polarToRectangular"])? },
        "distort.shear" => {
            let v = p.numbers("points", -1.0, 1.0)?;
            if v.len() < 4 || v.len() % 2 != 0 || v.len() > 64 {
                return Err(p.err("points", "2–32 \"t,offset\" pairs"));
            }
            let mut points: Vec<[f32; 2]> =
                v.chunks_exact(2).map(|c| [c.first().copied().unwrap_or(0.0).clamp(0.0, 1.0), c.get(1).copied().unwrap_or(0.0)]).collect();
            points.sort_by(|a, b| a[0].total_cmp(&b[0]));
            FilterParams::Shear { points, undefined: p.pick("undefined", UNDEFINED)? }
        }
        "distort.rasterZigZag" => FilterParams::ZigZag {
            amount: p.num("amount", -100.0, 100.0)?,
            ridges: p.num("ridges", 0.0, 20.0)?,
            style: p.pick("style", &["aroundCenter", "outFromCenter", "pondRipples"])?,
        },
        "other.highPass" => FilterParams::HighPass { radius: p.len("radius", 0.1, 250.0, MAX_REACH_PX)?.max(0.1) },
        "other.minimum" => FilterParams::Minimum { radius: p.len("radius", 0.1, 100.0, 100.0)?, preserve: p.pick("preserve", PRESERVE)? },
        "other.maximum" => FilterParams::Maximum { radius: p.len("radius", 0.1, 100.0, 100.0)?, preserve: p.pick("preserve", PRESERVE)? },
        "other.offset" => FilterParams::Offset {
            horizontal: p.len("horizontal", -5000.0, 5000.0, 5000.0)?.round() as i32,
            vertical: p.len("vertical", -5000.0, 5000.0, 5000.0)?.round() as i32,
            undefined: p.pick("undefined", UNDEFINED)?,
        },
        "other.custom" => {
            let kernel = p.numbers("kernel", -999.0, 999.0)?;
            if kernel.len() != 25 {
                return Err(p.err("kernel", "25 numbers (a 5×5 kernel)"));
            }
            FilterParams::Custom { kernel, scale: p.num("scale", 1.0, 9999.0)?, offset: p.num("offset", -9999.0, 9999.0)? }
        }
        "other.hsbHsl" => FilterParams::HsbHsl { input: p.pick("input", &["rgb", "hsb", "hsl"])?, output: p.pick("output", &["rgb", "hsb", "hsl"])? },
        "other.invert" => FilterParams::Invert,
        "other.desaturate" => FilterParams::Desaturate,
        "video.deInterlace" => FilterParams::DeInterlace {
            eliminate_even: p.choice("eliminate", &["odd", "even"])? == "even",
            interpolate: p.choice("createBy", &["interpolation", "duplication"])? == "interpolation",
        },
        "video.ntscColors" => FilterParams::NtscColors,
        _ => match id.strip_prefix("gallery.").and_then(GalleryFilter::from_key) {
            Some(filter) => FilterParams::FilterGallery { effects: vec![gallery_effect(filter, p)?] },
            None => return Err(format!("`{id}` is not a raster filter effect")),
        },
    })
}

/// A gallery filter's effect layer from the effect's parameters.
fn gallery_effect(filter: GalleryFilter, p: &P) -> Result<GalleryEffect, String> {
    let mut e = GalleryEffect::new(filter);
    for param in filter.params() {
        match &param.kind {
            GalleryParamKind::Range { min, max, .. } => {
                let v = p.num(param.key, *min as f64, *max as f64)?;
                e.set(param.key, v);
            }
            GalleryParamKind::Choice(options) => {
                let name = p.choice(param.key, options)?;
                if !e.set_choice(param.key, name) {
                    return Err(p.err(param.key, &format!("one of {}", options.join("|"))));
                }
            }
            GalleryParamKind::Bool(_) => {
                let b = p.flag(param.key)?;
                e.set(param.key, f32::from(u8::from(b)));
            }
        }
    }
    let doc = filter.params_doc();
    if doc.contains("\"foreground\":json") {
        e.foreground = p.color("foreground")?;
    }
    if doc.contains("\"background\":json") {
        e.background = p.color("background")?;
    }
    if doc.contains("\"glowColor\":json") {
        e.color = p.color("glowColor")?;
    }
    Ok(e)
}

/// Largest raster [`run_filter`] takes, in pixels (4096²).
pub const MAX_FILTER_PIXELS: u64 = 1 << 24;

/// A finite sample in 0..=1 (NaN and infinities → 0).
fn unit(v: f32) -> f32 {
    if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 }
}

/// Run `filter` on `pixels`, `width` × `height` premultiplied sRGB RGBA8 (the renderer's), in
/// place. `bounds` (pixels, `[x0, y0, x1, y1]`) is the art's box: distortions centre on it and
/// keep within it. The pixels become a straight-alpha float surface for the filter and are
/// premultiplied again afterwards. Errors (nothing changed) for mismatched or oversized input.
pub fn run_filter(pixels: &mut [u8], width: u32, height: u32, filter: &FilterParams, bounds: [i32; 4]) -> Result<(), String> {
    let count = u64::from(width) * u64::from(height);
    if width == 0 || height == 0 || count > MAX_FILTER_PIXELS || width > i32::MAX as u32 || height > i32::MAX as u32 {
        return Err(format!("raster effect size {width}×{height} out of range"));
    }
    if pixels.len() as u64 != count * 4 {
        return Err("raster effect pixel buffer has the wrong size".into());
    }
    let whole = PRect::new(0, 0, width as i32, height as i32);
    let [x0, y0, x1, y1] = bounds;
    let mut b = PRect::new(x0, y0, x1, y1).intersect(&whole);
    if b.is_empty() {
        b = whole;
    }
    // Premultiplied 8-bit → straight float.
    let mut data = Vec::with_capacity(pixels.len());
    for px in pixels.chunks_exact(4) {
        let a = px.get(3).copied().unwrap_or(0);
        if a == 0 {
            data.extend_from_slice(&[0.0; 4]);
            continue;
        }
        let af = a as f32 / 255.0;
        for c in px.iter().take(3) {
            data.push(unit(*c as f32 / 255.0 / af));
        }
        data.push(af);
    }
    let mut surface = Surface::new(PixelFormat::RGBA32F);
    surface.write_region(whole, &data);
    // The raster already holds the art plus the filter's reach: neighbourhood filters write all of
    // it, distortions their bounds.
    let area = photocraft_algo::output_area(filter, whole, b, None).intersect(&whole);
    if area.is_empty() {
        return Ok(());
    }
    let out = photocraft_algo::apply(&surface, filter, area, b, None);
    let res = out.read_region(whole);
    if res.len() != pixels.len() {
        return Err("raster effect output has the wrong size".into());
    }
    // Straight float → premultiplied 8-bit.
    for (px, v) in pixels.chunks_exact_mut(4).zip(res.chunks_exact(4)) {
        let a = unit(v.get(3).copied().unwrap_or(0.0));
        for (o, c) in px.iter_mut().zip(v.iter().take(3)) {
            *o = (unit(*c) * a * 255.0).round() as u8;
        }
        if let Some(o) = px.get_mut(3) {
            *o = (a * 255.0).round() as u8;
        }
    }
    Ok(())
}
