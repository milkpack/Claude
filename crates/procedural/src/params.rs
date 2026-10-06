//! Node parameters: their machine-readable specs (for the catalogue and generic UIs) and a reader
//! that checks, clamps and defaults untrusted values.

use serde_json::{Map, Value, json};
use vectorcraft_color::{Color, Paint};

/// What kind of value a parameter takes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ParamKind {
    /// A real number, clamped to `min..=max`.
    Number {
        default: f64,
        min: f64,
        max: f64,
    },
    /// A whole number, clamped to `min..=max` (fractions are rounded).
    Int {
        default: i64,
        min: i64,
        max: i64,
    },
    Bool {
        default: bool,
    },
    /// One of `options`.
    Choice {
        default: &'static str,
        options: &'static [&'static str],
    },
    /// A colour (`"#rrggbb"`, `[r, g, b]` 0..1) or `"none"` / null; `None` default = none.
    Color {
        default: Option<&'static str>,
    },
    /// A list of colours.
    Palette {
        default: &'static [&'static str],
    },
}

/// One parameter of a node kind.
#[derive(Clone, Copy, Debug)]
pub struct ParamSpec {
    pub name: &'static str,
    pub label: &'static str,
    pub kind: ParamKind,
    pub doc: &'static str,
}

/// The most colours a palette param keeps.
pub const MAX_PALETTE: usize = 64;

impl ParamSpec {
    pub const fn num(name: &'static str, label: &'static str, default: f64, min: f64, max: f64, doc: &'static str) -> Self {
        Self { name, label, kind: ParamKind::Number { default, min, max }, doc }
    }
    pub const fn int(name: &'static str, label: &'static str, default: i64, min: i64, max: i64, doc: &'static str) -> Self {
        Self { name, label, kind: ParamKind::Int { default, min, max }, doc }
    }
    pub const fn bool(name: &'static str, label: &'static str, default: bool, doc: &'static str) -> Self {
        Self { name, label, kind: ParamKind::Bool { default }, doc }
    }
    pub const fn choice(name: &'static str, label: &'static str, default: &'static str, options: &'static [&'static str], doc: &'static str) -> Self {
        Self { name, label, kind: ParamKind::Choice { default, options }, doc }
    }
    pub const fn color(name: &'static str, label: &'static str, default: Option<&'static str>, doc: &'static str) -> Self {
        Self { name, label, kind: ParamKind::Color { default }, doc }
    }
    pub const fn palette(name: &'static str, label: &'static str, default: &'static [&'static str], doc: &'static str) -> Self {
        Self { name, label, kind: ParamKind::Palette { default }, doc }
    }

    /// The default as JSON.
    pub fn default_value(&self) -> Value {
        match self.kind {
            ParamKind::Number { default, .. } => json!(default),
            ParamKind::Int { default, .. } => json!(default),
            ParamKind::Bool { default } => json!(default),
            ParamKind::Choice { default, .. } => json!(default),
            ParamKind::Color { default } => default.map_or(json!("none"), |c| json!(c)),
            ParamKind::Palette { default } => json!(default),
        }
    }

    /// The catalogue entry (`{name, label, type, default, min?, max?, options?, doc}`).
    pub fn to_json(&self) -> Value {
        let mut o = json!({ "name": self.name, "label": self.label, "default": self.default_value(), "doc": self.doc });
        let (ty, extra) = match self.kind {
            ParamKind::Number { min, max, .. } => ("number", json!({ "min": min, "max": max })),
            ParamKind::Int { min, max, .. } => ("integer", json!({ "min": min, "max": max })),
            ParamKind::Bool { .. } => ("bool", json!({})),
            ParamKind::Choice { options, .. } => ("choice", json!({ "options": options })),
            ParamKind::Color { .. } => ("color", json!({})),
            ParamKind::Palette { .. } => ("palette", json!({})),
        };
        if let (Some(m), Some(e)) = (o.as_object_mut(), extra.as_object()) {
            m.insert("type".into(), json!(ty));
            m.extend(e.clone());
        }
        o
    }

    /// Check a value for this parameter → the value as it will be stored (numbers clamped, a
    /// colour normalised to `#rrggbb` or `"none"`), or why it is not acceptable.
    pub fn check(&self, v: &Value) -> Result<Value, String> {
        let name = self.name;
        match self.kind {
            ParamKind::Number { min, max, .. } => {
                let x = v.as_f64().filter(|x| x.is_finite()).ok_or_else(|| format!("`{name}` must be a finite number"))?;
                Ok(json!(x.clamp(min, max)))
            }
            ParamKind::Int { min, max, .. } => {
                let x = v.as_f64().filter(|x| x.is_finite()).ok_or_else(|| format!("`{name}` must be a whole number"))?;
                Ok(json!((x.round().clamp(min as f64, max as f64)) as i64))
            }
            ParamKind::Bool { .. } => v.as_bool().map(Value::Bool).ok_or_else(|| format!("`{name}` must be true or false")),
            ParamKind::Choice { options, .. } => {
                let s = v.as_str().ok_or_else(|| format!("`{name}` must be one of {}", options.join(", ")))?;
                options
                    .iter()
                    .find(|o| **o == s)
                    .map(|o| json!(o))
                    .ok_or_else(|| format!("`{name}` must be one of {} (got `{}`)", options.join(", "), clip(s)))
            }
            ParamKind::Color { .. } => match parse_color(v) {
                Some(Some(c)) => Ok(json!(hex(c))),
                Some(None) => Ok(json!("none")),
                None => Err(format!("`{name}` must be a colour (\"#rrggbb\", [r, g, b] in 0..1) or \"none\"")),
            },
            ParamKind::Palette { .. } => {
                let a = v.as_array().ok_or_else(|| format!("`{name}` must be a list of colours"))?;
                if a.is_empty() || a.len() > MAX_PALETTE {
                    return Err(format!("`{name}` takes 1 to {MAX_PALETTE} colours"));
                }
                a.iter()
                    .map(|c| match parse_color(c) {
                        Some(Some(c)) => Ok(json!(hex(c))),
                        _ => Err(format!("`{name}`: every entry must be a colour")),
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map(Value::Array)
            }
        }
    }
}

/// `#rrggbb` without colour management (the same text whatever the colour settings).
pub fn hex(c: Color) -> String {
    let [r, g, b] = c.to_rgb_uncalibrated();
    let q = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", q(r), q(g), q(b))
}

fn clip(s: &str) -> String {
    s.chars().take(40).collect()
}

/// A colour value: `Some(None)` for none, `None` when it isn't a colour.
pub fn parse_color(v: &Value) -> Option<Option<Color>> {
    match v {
        Value::Null => Some(None),
        Value::String(s) if s.eq_ignore_ascii_case("none") || s.is_empty() => Some(None),
        Value::String(s) => Color::from_hex(s).map(Some),
        Value::Array(a) if a.len() == 3 => {
            let mut c = [0f32; 3];
            for (i, x) in a.iter().enumerate() {
                let x = x.as_f64().filter(|x| x.is_finite())?;
                *c.get_mut(i)? = x.clamp(0.0, 1.0) as f32;
            }
            Some(Some(Color::rgb(c[0], c[1], c[2])))
        }
        _ => None,
    }
}

/// Validate every param of a node against its specs: unknown names and bad values are errors.
pub fn validate(specs: &[ParamSpec], params: &Map<String, Value>) -> Result<(), String> {
    for (k, v) in params {
        let s = specs.iter().find(|s| s.name == k).ok_or_else(|| format!("unknown parameter `{}`", clip(k)))?;
        s.check(v)?;
    }
    Ok(())
}

/// Reads a node's params (already validated; anything off falls back to the default).
pub struct Reader<'a> {
    pub specs: &'static [ParamSpec],
    pub map: &'a Map<String, Value>,
}

impl Reader<'_> {
    fn spec(&self, name: &str) -> Option<&'static ParamSpec> {
        self.specs.iter().find(|s| s.name == name)
    }
    fn checked(&self, name: &str) -> Option<Value> {
        let s = self.spec(name)?;
        self.map.get(name).and_then(|v| s.check(v).ok())
    }
    pub fn num(&self, name: &str) -> f64 {
        let Some(s) = self.spec(name) else { return 0.0 };
        let d = match s.kind {
            ParamKind::Number { default, .. } => default,
            ParamKind::Int { default, .. } => default as f64,
            _ => 0.0,
        };
        self.checked(name).and_then(|v| v.as_f64()).unwrap_or(d)
    }
    pub fn int(&self, name: &str) -> i64 {
        let Some(s) = self.spec(name) else { return 0 };
        let d = match s.kind {
            ParamKind::Int { default, .. } => default,
            _ => 0,
        };
        self.checked(name).and_then(|v| v.as_i64()).unwrap_or(d)
    }
    /// A count (a non-negative int).
    pub fn count(&self, name: &str) -> usize {
        usize::try_from(self.int(name).max(0)).unwrap_or(0)
    }
    pub fn bool(&self, name: &str) -> bool {
        let d = matches!(self.spec(name).map(|s| s.kind), Some(ParamKind::Bool { default: true }));
        self.checked(name).and_then(|v| v.as_bool()).unwrap_or(d)
    }
    pub fn choice(&self, name: &str) -> &'static str {
        let Some(s) = self.spec(name) else { return "" };
        let ParamKind::Choice { default, options } = s.kind else { return "" };
        self.map.get(name).and_then(Value::as_str).and_then(|v| options.iter().find(|o| **o == v).copied()).unwrap_or(default)
    }
    /// A colour param as a paint (`Paint::None` for none).
    pub fn paint(&self, name: &str) -> Paint {
        self.color(name).map_or(Paint::None, Paint::solid)
    }
    pub fn color(&self, name: &str) -> Option<Color> {
        let s = self.spec(name)?;
        let ParamKind::Color { default } = s.kind else { return None };
        match self.map.get(name).and_then(parse_color) {
            Some(c) => c,
            None => default.and_then(Color::from_hex),
        }
    }
    pub fn palette(&self, name: &str) -> Vec<Color> {
        let Some(s) = self.spec(name) else { return vec![] };
        let ParamKind::Palette { default } = s.kind else { return vec![] };
        let given: Option<Vec<Color>> = self
            .map
            .get(name)
            .and_then(Value::as_array)
            .and_then(|a| a.iter().take(MAX_PALETTE).map(|c| parse_color(c).flatten()).collect::<Option<Vec<_>>>().filter(|v| !v.is_empty()));
        given.unwrap_or_else(|| default.iter().filter_map(|c| Color::from_hex(c)).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPECS: &[ParamSpec] = &[
        ParamSpec::num("w", "Width", 10.0, 0.0, 100.0, ""),
        ParamSpec::int("n", "Count", 3, 1, 9, ""),
        ParamSpec::bool("b", "Flag", true, ""),
        ParamSpec::choice("m", "Mode", "a", &["a", "b"], ""),
        ParamSpec::color("c", "Colour", Some("#ff0000"), ""),
        ParamSpec::palette("p", "Palette", &["#000000", "#ffffff"], ""),
    ];

    fn map(v: Value) -> Map<String, Value> {
        v.as_object().cloned().unwrap()
    }

    #[test]
    fn checks_clamp_and_reject() {
        let s = &SPECS[0];
        assert_eq!(s.check(&json!(1e9)).unwrap(), json!(100.0));
        assert!(s.check(&json!("x")).is_err());
        assert_eq!(SPECS[1].check(&json!(4.6)).unwrap(), json!(5));
        assert_eq!(SPECS[1].check(&json!(-1e300)).unwrap(), json!(1));
        assert!(SPECS[3].check(&json!("z")).is_err());
        assert_eq!(SPECS[4].check(&json!("#00FF00")).unwrap(), json!("#00ff00"));
        assert_eq!(SPECS[4].check(&json!(null)).unwrap(), json!("none"));
        assert_eq!(SPECS[4].check(&json!([0, 0, 2])).unwrap(), json!("#0000ff"));
        assert!(SPECS[4].check(&json!([0, "a", 2])).is_err());
        assert!(SPECS[5].check(&json!([])).is_err());
        assert!(SPECS[5].check(&json!(vec!["#000"; 100])).is_err());
        assert!(validate(SPECS, &map(json!({"zz": 1}))).is_err());
        assert!(validate(SPECS, &map(json!({"w": 5, "m": "b"}))).is_ok());
    }

    #[test]
    fn reader_defaults_and_clamps() {
        let m = map(json!({"w": 1e9, "n": "x", "b": false, "m": "q", "c": "none", "p": ["#123456"]}));
        let r = Reader { specs: SPECS, map: &m };
        assert_eq!(r.num("w"), 100.0);
        assert_eq!(r.int("n"), 3);
        assert!(!r.bool("b"));
        assert_eq!(r.choice("m"), "a");
        assert_eq!(r.color("c"), None);
        assert_eq!(r.palette("p").len(), 1);
        assert_eq!(r.num("missing"), 0.0);
        let e = Map::new();
        let r = Reader { specs: SPECS, map: &e };
        assert_eq!(r.color("c"), Color::from_hex("#ff0000"));
        assert_eq!(r.palette("p").len(), 2);
        assert!(r.bool("b"));
        assert_eq!(r.count("n"), 3);
    }
}
