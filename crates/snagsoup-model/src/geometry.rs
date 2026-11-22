use crate::error::{ModelError, Result};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct PointF {
    pub x: f64,
    pub y: f64,
}

impl PointF {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    pub fn to_snagit_string(&self) -> String {
        format!("{:.6},{:.6}", self.x, self.y)
    }

    pub fn from_snagit_str(s: &str) -> Result<Self> {
        let mut parts = s.split(',');
        let x_str = parts
            .next()
            .ok_or_else(|| ModelError::InvalidCoordinate(s.to_string()))?;
        let y_str = parts
            .next()
            .ok_or_else(|| ModelError::InvalidCoordinate(s.to_string()))?;
        if parts.next().is_some() {
            return Err(ModelError::InvalidCoordinate(s.to_string()));
        }
        let x = x_str
            .trim()
            .parse::<f64>()
            .map_err(|_| ModelError::InvalidCoordinate(s.to_string()))?;
        let y = y_str
            .trim()
            .parse::<f64>()
            .map_err(|_| ModelError::InvalidCoordinate(s.to_string()))?;
        Ok(Self { x, y })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct RectF {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl RectF {
    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn from_points(p1: PointF, p2: PointF) -> Self {
        let x = p1.x.min(p2.x);
        let y = p1.y.min(p2.y);
        let width = (p2.x - p1.x).abs();
        let height = (p2.y - p1.y).abs();
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn right(&self) -> f64 {
        self.x + self.width
    }

    pub fn bottom(&self) -> f64 {
        self.y + self.height
    }

    pub fn contains(&self, p: PointF) -> bool {
        p.x >= self.x && p.x <= self.right() && p.y >= self.y && p.y <= self.bottom()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SizeU {
    pub width: u32,
    pub height: u32,
}

impl SizeU {
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }
}

/// RGBA color supporting Snagit's `#AARRGGBB` (and `#RRGGBB`) hex format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ColorRgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl ColorRgba {
    pub const TRANSPARENT: Self = Self {
        r: 0,
        g: 0,
        b: 0,
        a: 0,
    };
    pub const BLACK: Self = Self {
        r: 0,
        g: 0,
        b: 0,
        a: 255,
    };
    pub const WHITE: Self = Self {
        r: 255,
        g: 255,
        b: 255,
        a: 255,
    };

    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    /// Formats as Snagit's canonical `#AARRGGBB` uppercase hex string.
    pub fn to_argb_hex(&self) -> String {
        format!("#{:02X}{:02X}{:02X}{:02X}", self.a, self.r, self.g, self.b)
    }

    /// Parses `#AARRGGBB` or `#RRGGBB` (with or without leading `#`).
    pub fn from_hex(s: &str) -> Result<Self> {
        let hex = s.strip_prefix('#').unwrap_or(s);
        match hex.len() {
            8 => {
                let a = u8::from_str_radix(&hex[0..2], 16)
                    .map_err(|_| ModelError::InvalidColor(s.to_string()))?;
                let r = u8::from_str_radix(&hex[2..4], 16)
                    .map_err(|_| ModelError::InvalidColor(s.to_string()))?;
                let g = u8::from_str_radix(&hex[4..6], 16)
                    .map_err(|_| ModelError::InvalidColor(s.to_string()))?;
                let b = u8::from_str_radix(&hex[6..8], 16)
                    .map_err(|_| ModelError::InvalidColor(s.to_string()))?;
                Ok(Self { r, g, b, a })
            }
            6 => {
                let r = u8::from_str_radix(&hex[0..2], 16)
                    .map_err(|_| ModelError::InvalidColor(s.to_string()))?;
                let g = u8::from_str_radix(&hex[2..4], 16)
                    .map_err(|_| ModelError::InvalidColor(s.to_string()))?;
                let b = u8::from_str_radix(&hex[4..6], 16)
                    .map_err(|_| ModelError::InvalidColor(s.to_string()))?;
                Ok(Self { r, g, b, a: 255 })
            }
            _ => Err(ModelError::InvalidColor(s.to_string())),
        }
    }
}

impl fmt::Display for ColorRgba {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02X}{:02X}{:02X}{:02X}", self.a, self.r, self.g, self.b)
    }
}

impl FromStr for ColorRgba {
    type Err = ModelError;

    fn from_str(s: &str) -> Result<Self> {
        Self::from_hex(s)
    }
}

impl Serialize for ColorRgba {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_argb_hex())
    }
}

impl<'de> Deserialize<'de> for ColorRgba {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        ColorRgba::from_hex(&s).map_err(serde::de::Error::custom)
    }
}

/// Serde helper for Snagit coordinate string `"x,y"`.
pub mod point_str_serde {
    use super::PointF;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(
        p: &PointF,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&p.to_snagit_string())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<PointF, D::Error> {
        let s = String::deserialize(deserializer)?;
        PointF::from_snagit_str(&s).map_err(serde::de::Error::custom)
    }
}

/// Serde helper for `Option<PointF>` encoded as `"x,y"`.
pub mod opt_point_str_serde {
    use super::PointF;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(
        p: &Option<PointF>,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        match p {
            Some(pt) => serializer.serialize_str(&pt.to_snagit_string()),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Option<PointF>, D::Error> {
        let opt = Option::<String>::deserialize(deserializer)?;
        match opt {
            Some(s) if !s.trim().is_empty() => {
                PointF::from_snagit_str(&s).map(Some).map_err(serde::de::Error::custom)
            }
            _ => Ok(None),
        }
    }
}

/// Serde helper for `Option<Vec<PointF>>` encoded as `["x1,y1", "x2,y2", ...]`.
pub mod opt_vec_point_str_serde {
    use super::PointF;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(
        pts: &Option<Vec<PointF>>,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        match pts {
            Some(v) => {
                let strings: Vec<String> = v.iter().map(|p| p.to_snagit_string()).collect();
                strings.serialize(serializer)
            }
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Option<Vec<PointF>>, D::Error> {
        let opt = Option::<Vec<String>>::deserialize(deserializer)?;
        match opt {
            Some(v) => {
                let mut out = Vec::with_capacity(v.len());
                for s in v {
                    out.push(PointF::from_snagit_str(&s).map_err(serde::de::Error::custom)?);
                }
                Ok(Some(out))
            }
            None => Ok(None),
        }
    }
}
