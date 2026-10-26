//! SyncTeX's map of a build: which line of which file made which box on
//! which page, read from the `.synctex.gz` TeX writes beside the PDF — and
//! searched both ways, from a line to where it is in the PDF (forward) and
//! from a point on a page to the line that put something there (inverse).
//!
//! The file is text, gzipped: a preamble naming the inputs by tag, then
//! each page's records — boxes opened with `[` (vertical) or `(`
//! (horizontal) and closed, void boxes `v` and `h`, and points `x`, `k`
//! (kern), `g` (glue) and `$` (math):
//!
//! ```text
//! Input:1:/Users/you/paper/main.tex
//! Unit:1
//! Content:
//! {1
//! (1,12:4736286,6974604:30785863,655360,183500
//! g1,12:4736286,6974604
//! )
//! }1
//! ```
//!
//! Each record is `tag,line[,column]:h,v` and, for a box, `:width,height,depth`
//! — in TeX's scaled points times `Unit`, from the page's top left, `v` down
//! to the baseline. Points here are PDF points (big points), as the viewer
//! draws them.

use std::collections::HashMap;
use std::io::Read;

use serde::{Deserialize, Serialize};

/// Scaled points to a big point: 65536 × 72.27 / 72.
const SP_PER_BP: f64 = 65781.76;

/// A rectangle on a page, in PDF points from its top left.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// What a record is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// A box: horizontal or vertical, opened or void.
    Hbox,
    Vbox,
    /// A point: kern, glue, math, the current position.
    Point,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Record {
    kind: Kind,
    tag: u32,
    line: u32,
    /// In big points: the reference point, and the box's extent.
    h: f64,
    v: f64,
    width: f64,
    height: f64,
    depth: f64,
}

impl Record {
    /// The box's area on the page, or a small square round a point.
    fn rect(&self) -> Rect {
        match self.kind {
            Kind::Point => Rect {
                x: (self.h - 2.0) as f32,
                y: (self.v - 8.0) as f32,
                w: 4.0,
                h: 10.0,
            },
            _ => Rect {
                x: self.h as f32,
                y: (self.v - self.height) as f32,
                w: self.width.abs() as f32,
                h: (self.height + self.depth) as f32,
            },
        }
    }

    fn contains(&self, x: f64, y: f64) -> bool {
        let r = self.rect();
        x >= r.x as f64 && x <= (r.x + r.w) as f64 && y >= r.y as f64 && y <= (r.y + r.h) as f64
    }
}

/// A build's SyncTeX map.
#[derive(Debug, Clone, Default)]
pub struct Map {
    /// The files, by tag, as TeX named them.
    inputs: HashMap<u32, String>,
    /// Each page's records, pages counted from one.
    pages: Vec<Vec<Record>>,
}

/// Where a line is in the PDF.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Found {
    pub page: u32,
    pub rects: Vec<Rect>,
}

/// Where a point on a page came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    /// The file, as TeX named it.
    pub file: String,
    /// One-based.
    pub line: u32,
}

/// `a,b` or `a,b,c` — the numbers before a record's first colon.
fn tag_line(text: &str) -> Option<(u32, u32)> {
    let mut parts = text.split(',');
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

fn numbers(text: &str) -> Vec<f64> {
    text.split(',')
        .filter_map(|n| n.trim().parse::<f64>().ok())
        .collect()
}

impl Map {
    /// Read a `.synctex.gz`'s bytes, or a plain `.synctex`'s.
    pub fn read(bytes: &[u8]) -> Result<Map, String> {
        let text = if bytes.starts_with(&[0x1f, 0x8b]) {
            let mut out = String::new();
            flate2::read::GzDecoder::new(bytes)
                .read_to_string(&mut out)
                .map_err(|e| format!("SyncTeX's file did not unpack: {e}"))?;
            out
        } else {
            String::from_utf8_lossy(bytes).to_string()
        };
        Ok(Map::parse(&text))
    }

    /// Read SyncTeX's text.
    pub fn parse(text: &str) -> Map {
        let mut map = Map::default();
        let (mut unit, mut x_offset, mut y_offset, mut mag) =
            (1.0_f64, 0.0_f64, 0.0_f64, 1000.0_f64);
        let mut content = false;
        let mut page: Option<usize> = None;
        let scale = |value: f64, unit: f64, mag: f64| value * unit * (mag / 1000.0) / SP_PER_BP;
        for line in text.lines() {
            if !content {
                if let Some(rest) = line.strip_prefix("Input:") {
                    if let Some((tag, file)) = rest.split_once(':') {
                        if let Ok(tag) = tag.parse() {
                            map.inputs.insert(tag, file.to_string());
                        }
                    }
                } else if let Some(v) = line.strip_prefix("Unit:") {
                    unit = v.trim().parse().unwrap_or(1.0);
                } else if let Some(v) = line.strip_prefix("X Offset:") {
                    x_offset = v.trim().parse().unwrap_or(0.0);
                } else if let Some(v) = line.strip_prefix("Y Offset:") {
                    y_offset = v.trim().parse().unwrap_or(0.0);
                } else if let Some(v) = line.strip_prefix("Magnification:") {
                    mag = v.trim().parse().unwrap_or(1000.0);
                } else if line.starts_with("Content:") {
                    content = true;
                }
                continue;
            }
            // Inputs can be named mid-content too.
            if let Some(rest) = line.strip_prefix("Input:") {
                if let Some((tag, file)) = rest.split_once(':') {
                    if let Ok(tag) = tag.parse() {
                        map.inputs.insert(tag, file.to_string());
                    }
                }
                continue;
            }
            if line.starts_with("Postamble:") {
                break;
            }
            let Some(first) = line.chars().next() else {
                continue;
            };
            match first {
                '{' => {
                    let n: usize = line[1..].trim().parse().unwrap_or(map.pages.len() + 1);
                    while map.pages.len() < n {
                        map.pages.push(Vec::new());
                    }
                    page = Some(n - 1);
                }
                '}' => page = None,
                '[' | '(' | 'v' | 'h' | 'x' | 'k' | 'g' | '$' => {
                    let Some(at) = page else { continue };
                    let body = &line[1..];
                    let mut fields = body.split(':');
                    let (Some(tl), Some(point)) = (fields.next(), fields.next()) else {
                        continue;
                    };
                    let Some((tag, number)) = tag_line(tl) else {
                        continue;
                    };
                    let p = numbers(point);
                    if p.len() < 2 {
                        continue;
                    }
                    let size = fields.next().map(numbers).unwrap_or_default();
                    let kind = match first {
                        '[' | 'v' => Kind::Vbox,
                        '(' | 'h' => Kind::Hbox,
                        _ => Kind::Point,
                    };
                    let get = |i: usize| size.get(i).copied().unwrap_or(0.0);
                    map.pages[at].push(Record {
                        kind,
                        tag,
                        line: number,
                        h: scale(p[0], unit, mag) + x_offset / SP_PER_BP,
                        v: scale(p[1], unit, mag) + y_offset / SP_PER_BP,
                        width: scale(get(0), unit, mag),
                        height: scale(get(1), unit, mag),
                        depth: scale(get(2), unit, mag),
                    });
                }
                _ => {}
            }
        }
        map
    }

    pub fn pages(&self) -> usize {
        self.pages.len()
    }

    /// The tags of the inputs a file is, matched as the end of TeX's name
    /// for it: `chapters/intro.tex` is `/Users/you/paper/./chapters/intro.tex`.
    fn tags_of(&self, file: &str) -> Vec<u32> {
        let want = normalise(file);
        self.inputs
            .iter()
            .filter(|(_, name)| {
                let name = normalise(name);
                name == want
                    || name.ends_with(&format!("/{want}"))
                    || want.ends_with(&format!("/{name}"))
            })
            .map(|(tag, _)| *tag)
            .collect()
    }

    /// Where `line` of `file` is: the first page with records of it, and
    /// the boxes there — of the nearest line before it that has any, when
    /// it has none, as a blank line or a comment does not.
    pub fn forward(&self, file: &str, line: u32) -> Option<Found> {
        let tags = self.tags_of(file);
        if tags.is_empty() {
            return None;
        }
        for wanted in (line.saturating_sub(40).max(1)..=line)
            .rev()
            .chain(line + 1..line + 10)
        {
            for (index, records) in self.pages.iter().enumerate() {
                let hits: Vec<&Record> = records
                    .iter()
                    .filter(|r| tags.contains(&r.tag) && r.line == wanted && r.kind != Kind::Vbox)
                    .collect();
                if hits.is_empty() {
                    continue;
                }
                // The boxes, or the points when there are no boxes.
                let boxes: Vec<Rect> = hits
                    .iter()
                    .filter(|r| r.kind == Kind::Hbox && r.width > 0.0)
                    .map(|r| r.rect())
                    .collect();
                let rects = if boxes.is_empty() {
                    hits.iter().map(|r| r.rect()).collect()
                } else {
                    merge(boxes)
                };
                return Some(Found {
                    page: index as u32 + 1,
                    rects,
                });
            }
        }
        None
    }

    /// Where a point on `page` (one-based, in points from its top left) came
    /// from: the smallest box around it, else the nearest record.
    pub fn inverse(&self, page: u32, x: f64, y: f64) -> Option<Source> {
        let records = self.pages.get(page.checked_sub(1)? as usize)?;
        let around = records
            .iter()
            .filter(|r| r.kind == Kind::Hbox && r.contains(x, y))
            .min_by(|a, b| {
                (a.width * (a.height + a.depth)).total_cmp(&(b.width * (b.height + b.depth)))
            });
        let chosen = around.or_else(|| {
            records
                .iter()
                .filter(|r| r.kind != Kind::Vbox)
                .min_by(|a, b| {
                    let d = |r: &Record| {
                        let rect = r.rect();
                        let cy = (rect.y + rect.h / 2.0) as f64;
                        let dx = if x < rect.x as f64 {
                            rect.x as f64 - x
                        } else if x > (rect.x + rect.w) as f64 {
                            x - (rect.x + rect.w) as f64
                        } else {
                            0.0
                        };
                        // A line above or below is further than one beside.
                        (cy - y).abs() * 4.0 + dx
                    };
                    d(a).total_cmp(&d(b))
                })
        })?;
        Some(Source {
            file: self.inputs.get(&chosen.tag)?.clone(),
            line: chosen.line,
        })
    }
}

/// Boxes on one line run together: the rectangles merged where they touch
/// on the same baseline.
fn merge(mut rects: Vec<Rect>) -> Vec<Rect> {
    rects.sort_by(|a, b| a.y.total_cmp(&b.y).then(a.x.total_cmp(&b.x)));
    let mut out: Vec<Rect> = Vec::new();
    for r in rects {
        if let Some(last) = out.last_mut() {
            if (last.y - r.y).abs() < 1.0
                && (last.h - r.h).abs() < 2.0
                && r.x <= last.x + last.w + 2.0
            {
                let right = (last.x + last.w).max(r.x + r.w);
                last.w = right - last.x;
                continue;
            }
        }
        out.push(r);
    }
    out
}

/// A path with its `.` and `..` worked out, and `/./` taken out.
fn normalise(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    format!(
        "{}{}",
        if path.starts_with('/') { "/" } else { "" },
        parts.join("/")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two pages: on the first, line 5 of main.tex as a box 300 points wide
    /// at the top, and line 3 of intro.tex below it; on the second, line 9
    /// of main.tex. Numbers are scaled points: 65781.76 × points.
    fn fixture() -> String {
        let sp = |bp: f64| (bp * SP_PER_BP).round() as i64;
        format!(
            "SyncTeX Version:1\nInput:1:/Users/you/paper/main.tex\nInput:2:/Users/you/paper/./chapters/intro.tex\nOutput:pdf\nMagnification:1000\nUnit:1\nX Offset:0\nY Offset:0\nContent:\n\
             !100\n{{1\n[1,1:{a},{b}:{w},{h},0\n(1,5:{a},{c}:{w},{h10},{d2}\ng1,5:{a},{c}\n)\n(2,3:{a},{e}:{w2},{h10},{d2}\nk2,3:{f},{e}:{k}\n)\n]\n}}1\n\
             {{2\n[1,8:{a},{b}:{w},{h},0\n(1,9:{a},{c}:{w},{h10},{d2}\n)\n]\n}}2\nPostamble:\nCount:9\n",
            a = sp(72.0), b = sp(72.0), w = sp(300.0), h = sp(600.0), c = sp(100.0), h10 = sp(10.0), d2 = sp(2.0), e = sp(140.0), w2 = sp(200.0), f = sp(150.0), k = sp(3.0),
        )
    }

    #[test]
    fn a_line_is_found_on_its_page_with_its_box() {
        let map = Map::parse(&fixture());
        assert_eq!(map.pages(), 2);
        let found = map.forward("main.tex", 5).unwrap();
        assert_eq!(found.page, 1);
        let r = found.rects[0];
        assert!(
            (r.x - 72.0).abs() < 0.01
                && (r.y - 90.0).abs() < 0.01
                && (r.w - 300.0).abs() < 0.01
                && (r.h - 12.0).abs() < 0.01,
            "{r:?}"
        );
        assert_eq!(
            map.forward("chapters/intro.tex", 3).unwrap().page,
            1,
            "a file named relatively, TeX naming it with ./"
        );
        assert_eq!(map.forward("main.tex", 9).unwrap().page, 2);
        // A blank line is found where the line before it is.
        assert_eq!(
            map.forward("main.tex", 7).unwrap().rects,
            map.forward("main.tex", 5).unwrap().rects
        );
        assert!(map.forward("other.tex", 1).is_none());
    }

    #[test]
    fn a_point_on_a_page_is_found_in_its_line() {
        let map = Map::parse(&fixture());
        assert_eq!(
            map.inverse(1, 100.0, 95.0),
            Some(Source {
                file: "/Users/you/paper/main.tex".into(),
                line: 5
            })
        );
        assert_eq!(
            map.inverse(1, 100.0, 135.0),
            Some(Source {
                file: "/Users/you/paper/./chapters/intro.tex".into(),
                line: 3
            })
        );
        // Below everything: the nearest line.
        assert_eq!(map.inverse(1, 100.0, 300.0).unwrap().line, 3);
        assert_eq!(map.inverse(2, 100.0, 95.0).unwrap().line, 9);
        assert!(map.inverse(3, 1.0, 1.0).is_none());
    }

    #[test]
    fn a_gzipped_map_is_read_as_a_plain_one_is() {
        use std::io::Write;
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(fixture().as_bytes()).unwrap();
        let map = Map::read(&gz.finish().unwrap()).unwrap();
        assert_eq!(map.forward("main.tex", 9).unwrap().page, 2);
        assert!(Map::read(&[0x1f, 0x8b, 0, 1]).is_err());
    }
}
