// Quark's application icon: a baryon, three colour-charged quarks bound into
// a triangle on a violet tile.
//
// # Why it is drawn rather than loaded
//
// The icon has to exist in two places that cannot share a file: the running
// window sets it through winit, and Windows Explorer reads it from a resource
// compiled into the executable. Drawing it means both come from this one
// function — `build.rs` includes this file directly to generate the `.ico`, so
// the pinned icon and the window icon cannot drift apart.
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

/// Distance to a line segment, for the bonds between the quarks.
pub fn sd_segment(px: f32, py: f32, a: (f32, f32), b: (f32, f32)) -> f32 {
    let (vx, vy) = (b.0 - a.0, b.1 - a.1);
    let (wx, wy) = (px - a.0, py - a.1);
    let len2 = vx * vx + vy * vy;
    let t = if len2 > 0.0 {
        ((wx * vx + wy * vy) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (wx - vx * t).hypot(wy - vy * t)
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
/// RGBA, the layout both winit and the ICO format expect.
///
/// Every dimension is a fraction of `size`, so the mark is identical at the
/// 16px the taskbar uses and the 256px Explorer wants for large icons.
///
/// The three dots keep the red/green/blue of colour charge rather than the
/// brand purple — against the violet tile they are the only thing that reads
/// at 16px, and a monochrome triplet collapses into a smudge at that size.
pub fn render(size: usize) -> Vec<u8> {
    let s = size as f32;
    let c = (s - 1.0) / 2.0;

    let half = s * 0.455;
    let radius = s * 0.24;
    // Distance of each quark from the centre, and how big each one is.
    let orbit = s * 0.200;
    let dot = s * 0.083;
    let bond = s * 0.017;

    // Equilateral, point up: the triangle reads as deliberate at any rotation
    // but this is the one that looks upright in a taskbar.
    let quark = |deg: f32| -> (f32, f32) {
        let rad = deg.to_radians();
        (c + orbit * rad.cos(), c + orbit * rad.sin())
    };
    let quarks = [
        (quark(-90.0), rgb(0xFF7BA8)),
        (quark(30.0), rgb(0x5FE39B)),
        (quark(150.0), rgb(0x6FA8FF)),
    ];

    let tile_top = rgb(0x34205C);
    let tile_bottom = rgb(0x150C26);
    let bond_colour = rgb(0xC9B6FF);
    let glow = rgb(0xA855F7);

    let mut buf = vec![0f32; size * size * 4];
    for y in 0..size {
        for x in 0..size {
            let (px, py) = (x as f32, y as f32);
            let i = (y * size + x) * 4;

            // The tile, lit from the top so it sits in the same family as the
            // glass chrome.
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

            // A soft violet bloom under the triplet, so the dots look lit
            // rather than pasted on.
            let bloom = 1.0 - ((px - c).hypot(py - c) / (s * 0.42)).min(1.0);
            over(&mut buf, i, glow, bloom * bloom * 0.30 * inside);

            // Bonds first, so the dots sit on top of them.
            let mut bd = f32::MAX;
            for k in 0..3 {
                let a = quarks[k].0;
                let b = quarks[(k + 1) % 3].0;
                bd = bd.min(sd_segment(px, py, a, b));
            }
            over(&mut buf, i, bond_colour, coverage(bd - bond) * 0.45 * inside);

            for ((qx, qy), colour) in quarks {
                let dd = (px - qx).hypot(py - qy) - dot;
                over(&mut buf, i, colour, coverage(dd) * inside);
            }
        }
    }

    // The accumulator is premultiplied; both winit and ICO want straight alpha.
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
