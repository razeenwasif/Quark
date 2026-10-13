// Quark's application icon: the mark from the title bar — three quarks bound
// in a ring — on a velvet tile.
//
// # Why it is drawn rather than loaded
//
// The icon has to exist in several places that cannot share a file: the
// running window sets it through winit, Windows Explorer reads it from a
// resource compiled into the executable, and the macOS bundle carries an
// `.icns`. Drawing it means all of them come from this one function —
// `build.rs` and xtask include this file directly, so the pinned icon, the
// Dock icon and the window icon cannot drift apart, or away from the brand
// mark the title bar draws.
//
// This module deliberately has no dependencies, not even `egui`. `build.rs`
// pulls it in with `include!`, and a build script cannot see the crate's own
// dependency graph.

/// Signed distance to a rounded square centred on the origin.
pub fn sd_round_square(px: f32, py: f32, half: f32, r: f32) -> f32 {
    let qx = px.abs() - half + r;
    let qy = py.abs() - half + r;
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
}

/// Antialiased coverage from a signed distance, in pixels.
///
/// One pixel of falloff centred on the edge. Cheaper than supersampling and,
/// for shapes this simple, indistinguishable from it.
pub fn coverage(d: f32) -> f32 {
    (0.5 - d).clamp(0.0, 1.0)
}

/// Source-over one colour onto a premultiplied accumulator.
pub fn over(buf: &mut [f32], i: usize, rgb: [f32; 3], a: f32) {
    if a <= 0.0 {
        return;
    }
    let inv = 1.0 - a;
    buf[i] = rgb[0] * a + buf[i] * inv;
    buf[i + 1] = rgb[1] * a + buf[i + 1] * inv;
    buf[i + 2] = rgb[2] * a + buf[i + 2] * inv;
    buf[i + 3] = a + buf[i + 3] * inv;
}

pub const fn rgb(hex: u32) -> [f32; 3] {
    [
        ((hex >> 16) & 0xff) as f32 / 255.0,
        ((hex >> 8) & 0xff) as f32 / 255.0,
        (hex & 0xff) as f32 / 255.0,
    ]
}

/// Renders the icon at `size` pixels square as straight (non-premultiplied)
/// RGBA, the layout winit, the ICO format and PNG all expect.
///
/// Every dimension is a fraction of `size`, so the mark is identical at the
/// 16px a taskbar uses and the 1024px the Dock wants — with floors on the
/// ring and the dots, so neither rounds away to nothing at the small end.
///
/// The three quarks are told apart by lightness rather than hue — lilac, the
/// amethyst accent, and the accent dimmed — which is how the title bar's mark
/// is drawn, and what still separates them when hue is lost at 16px.
pub fn render(size: usize) -> Vec<u8> {
    let s = size as f32;
    let c = (s - 1.0) / 2.0;

    // The tile.
    let half = s * 0.455;
    let radius = s * 0.24;
    // The mark, at the proportions the title bar draws it, scaled to leave
    // the tile a margin round the ring.
    let ring = s * 0.29;
    let ring_width = (s * 0.05).max(1.2);
    let dot = (s * 0.068).max(1.1);

    let accent = rgb(0x9D7BFF);
    let quarks = [
        ((c, c - s * 0.145), accent, 1.0),
        ((c - s * 0.125, c + s * 0.075), rgb(0xC6B2FF), 1.0),
        ((c + s * 0.125, c + s * 0.075), accent, 0.55),
    ];

    // Velvet: a violet-black, lit softly from above.
    let tile_top = rgb(0x1E1A28);
    let tile_bottom = rgb(0x0E0D12);

    let mut buf = vec![0f32; size * size * 4];
    for y in 0..size {
        for x in 0..size {
            let (px, py) = (x as f32, y as f32);
            let i = (y * size + x) * 4;

            let d = sd_round_square(px - c, py - c, half, radius);
            let t = (py / s).clamp(0.0, 1.0);
            let fill = [
                tile_top[0] * (1.0 - t) + tile_bottom[0] * t,
                tile_top[1] * (1.0 - t) + tile_bottom[1] * t,
                tile_top[2] * (1.0 - t) + tile_bottom[2] * t,
            ];
            over(&mut buf, i, fill, coverage(d));

            // Everything below is clipped to the tile, so nothing spills past
            // the rounded corners.
            let inside = coverage(d);
            if inside <= 0.0 {
                continue;
            }

            // The light a velvet nap catches, falling from above the mark.
            let bloom = 1.0 - ((px - c).hypot(py - (c - s * 0.30)) / (s * 0.62)).min(1.0);
            over(&mut buf, i, accent, bloom * bloom * 0.16 * inside);

            // The ring, translucent, so it reads as the bond rather than as a
            // fourth thing competing with the quarks.
            let rd = ((px - c).hypot(py - c) - ring).abs() - ring_width / 2.0;
            over(&mut buf, i, accent, coverage(rd) * 0.45 * inside);

            for ((qx, qy), colour, alpha) in quarks {
                let dd = (px - qx).hypot(py - qy) - dot;
                over(&mut buf, i, colour, coverage(dd) * alpha * inside);
            }
        }
    }

    // The accumulator is premultiplied; winit, ICO and PNG want straight alpha.
    let mut rgba = vec![0u8; size * size * 4];
    for i in (0..buf.len()).step_by(4) {
        let a = buf[i + 3];
        let to_byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        if a > 0.0 {
            rgba[i] = to_byte(buf[i] / a);
            rgba[i + 1] = to_byte(buf[i + 1] / a);
            rgba[i + 2] = to_byte(buf[i + 2] / a);
        }
        rgba[i + 3] = to_byte(a);
    }
    rgba
}

/// The sizes baked into the `.ico`.
///
/// Windows picks the nearest and downscales; supplying the exact sizes it asks
/// for avoids a resample. 256 is what Explorer's large-icon views use, 16 and
/// 32 are the taskbar and title bar.
///
/// Consumed by `build.rs`, which includes this file to generate the icon. The
/// binary itself only ever renders one size, so its own dead-code analysis
/// cannot see that use.
#[allow(dead_code)]
pub const ICO_SIZES: [u32; 9] = [16, 20, 24, 32, 40, 48, 64, 128, 256];
