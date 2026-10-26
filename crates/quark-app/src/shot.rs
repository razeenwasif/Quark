//! Pictures of the interface, drawn off screen: egui's own frame, rendered
//! by egui-wgpu into a texture and written out as a PNG. For looking at a
//! screen without a window; run on purpose, with
//! `QUARK_SHOTS=<folder> cargo test -p quark-app shot -- --ignored`.

use std::path::{Path, PathBuf};

use eframe::egui_wgpu::{self, wgpu};

/// A native wgpu future: ready at once, so polled once with a waker that
/// does nothing.
fn ready<F: std::future::Future>(f: F) -> F::Output {
    let mut f = std::pin::pin!(f);
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    loop {
        if let std::task::Poll::Ready(out) = f.as_mut().poll(&mut cx) {
            return out;
        }
        std::thread::yield_now();
    }
}

/// Draw `frames` frames of `draw` at `size` points, twice over for Retina, and
/// write the last as `out`. The frames before it settle fonts and layout.
pub(crate) fn shoot(
    ctx: &egui::Context,
    size: egui::Vec2,
    frames: usize,
    out: &Path,
    mut draw: impl FnMut(&mut egui::Ui),
) -> Result<(), String> {
    const SCALE: f32 = 2.0;
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = ready(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).map_err(|e| e.to_string())?;
    let (device, queue) = ready(adapter.request_device(&wgpu::DeviceDescriptor::default())).map_err(|e| e.to_string())?;
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut renderer = egui_wgpu::Renderer::new(&device, format, egui_wgpu::RendererOptions::default());
    let px = [(size.x * SCALE) as u32, (size.y * SCALE) as u32];

    ctx.set_pixels_per_point(SCALE);
    let mut last = None;
    for frame in 0..frames.max(1) {
        // Half a second a frame, so fades and slides have finished.
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            time: Some(frame as f64 * 0.5),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| draw(ui));
        for (id, deltas) in &output.textures_delta.set {
            for delta in deltas {
                renderer.update_texture(&device, &queue, *id, delta);
            }
        }
        for id in &output.textures_delta.free {
            renderer.free_texture(id);
        }
        output.textures_delta.clear();
        last = Some(output);
    }
    let output = last.expect("at least a frame");
    let jobs = ctx.tessellate(output.shapes, output.pixels_per_point);
    let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: px, pixels_per_point: output.pixels_per_point };

    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("shot"),
        size: wgpu::Extent3d { width: px[0], height: px[1], depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let mut encoder = device.create_command_encoder(&Default::default());
    let mut buffers = renderer.update_buffers(&device, &queue, &mut encoder, &jobs, &screen);
    {
        let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("shot"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            occlusion_query_set: None,
            timestamp_writes: None,
            multiview_mask: None,
        });
        renderer.render(&mut pass.forget_lifetime(), &jobs, &screen);
    }
    // Rows are copied out padded to the 256 bytes wgpu asks for.
    let row = px[0] * 4;
    let padded = row.div_ceil(256) * 256;
    let read = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("shot"),
        size: u64::from(padded) * u64::from(px[1]),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        target.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &read,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded), rows_per_image: None },
        },
        target.size(),
    );
    buffers.push(encoder.finish());
    queue.submit(buffers);
    read.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| e.to_string())?;
    let mapped = read.slice(..).get_mapped_range().map_err(|e| format!("{e:?}"))?;
    let mut rgba = Vec::with_capacity((row * px[1]) as usize);
    for line in mapped.chunks(padded as usize) {
        rgba.extend_from_slice(&line[..row as usize]);
    }
    drop(mapped);
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    image::save_buffer(out, &rgba, px[0], px[1], image::ExtendedColorType::Rgba8).map_err(|e| e.to_string())
}

/// Where pictures go, when they are asked for.
pub(crate) fn folder() -> Option<PathBuf> {
    std::env::var_os("QUARK_SHOTS").map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use quark_pdf::library::Book;
    use quark_pdf::render::Raster;
    use quark_ui::theme::{self, Palette, ThemeMode};

    /// A cover of the test's own: a band of colour over a paler page.
    fn cover(w: u32, h: u32, top: [u8; 3], page: [u8; 3]) -> Raster {
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let band = y < h * 2 / 5 || (y > h * 3 / 4 && y < h * 3 / 4 + h / 30 && x > w / 8 && x < w * 7 / 8);
                let [r, g, b] = if band { top } else { page };
                rgba.extend_from_slice(&[r, g, b, 255]);
            }
        }
        Raster { width: w, height: h, rgba }
    }

    fn book(shelf: &str, title: &str, author: &str, pages: u32, page: Option<u32>, read: Option<u64>) -> Book {
        Book {
            path: PathBuf::from(format!("/books/{shelf}/{title}.pdf")),
            file: format!("{}.pdf", title.to_lowercase().replace(' ', "-")),
            title: title.into(),
            author: author.into(),
            pages,
            size: 1,
            shelf: shelf.into(),
            folder: "~/Documents/Books".into(),
            in_cloud: false,
            page,
            read,
            renamed: false,
        }
    }

    #[test]
    #[ignore = "draws pictures; run with QUARK_SHOTS set"]
    fn shot_the_library() {
        let Some(dir) = folder() else { return };
        let books = vec![
            book("", "Linear Algebra Done Right", "Sheldon Axler", 340, Some(118), Some(30)),
            book("", "Concrete Mathematics", "Graham, Knuth, Patashnik", 657, None, None),
            book("ML", "Mathematics for Machine Learning", "Deisenroth, Faisal, Ong", 417, Some(212), Some(50)),
            book("ML", "Pattern Recognition and Machine Learning", "Christopher Bishop", 758, None, None),
            book("ML", "Deep Learning", "Goodfellow, Bengio, Courville", 800, Some(40), Some(20)),
            Book { in_cloud: true, ..book("ML", "Information Theory, Inference, and Learning Algorithms", "", 640, None, None) },
            book("Physics", "The Feynman Lectures on Physics, Volume I", "Richard Feynman", 560, None, None),
            book("Physics", "Classical Mechanics", "", 0, None, None),
        ];
        let covers = [
            (0, [176, 40, 48], [246, 240, 228]),
            (2, [32, 72, 128], [236, 240, 246]),
            (3, [20, 20, 24], [232, 232, 232]),
            (4, [214, 120, 30], [250, 246, 236]),
            (6, [24, 96, 72], [240, 244, 238]),
        ];
        for (dark, name) in [(true, "library-dark.png"), (false, "library-light.png")] {
            let ctx = egui::Context::default();
            theme::install_fonts(&ctx);
            let mode = if dark { ThemeMode::Dark } else { ThemeMode::Light };
            theme::apply(&ctx, mode);
            let p = Palette::for_mode(mode);
            let mut home = crate::home::HomeState::default();
            home.books.books = books.clone();
            home.books.folders = vec!["~/Documents/Books".into()];
            for (i, top, page) in covers {
                home.books.cover_drawn(&ctx, books[i].path.clone(), Some(&cover(240, 340, top, page)));
            }
            let prefs = quark_core::prefs::Prefs::default();
            shoot(&ctx, egui::vec2(1320.0, 900.0), 6, &dir.join(name), |ui| {
                egui::CentralPanel::default().frame(egui::Frame::new().fill(p.ground).inner_margin(12)).show(ui, |ui| {
                    crate::home::show(ui, &p, &mut home, &prefs);
                });
            })
            .unwrap();
        }

        // Renaming, and words typed narrowing the shelves.
        let ctx = egui::Context::default();
        theme::install_fonts(&ctx);
        theme::apply(&ctx, ThemeMode::Dark);
        let p = Palette::for_mode(ThemeMode::Dark);
        let mut home = crate::home::HomeState::default();
        home.books.books = books.clone();
        home.books.folders = vec!["~/Documents/Books".into()];
        home.books.query = "learning".into();
        home.books.rename(&books[2]);
        let prefs = quark_core::prefs::Prefs::default();
        shoot(&ctx, egui::vec2(1320.0, 900.0), 6, &dir.join("library-rename.png"), |ui| {
            egui::CentralPanel::default().frame(egui::Frame::new().fill(p.ground).inner_margin(12)).show(ui, |ui| {
                crate::home::show(ui, &p, &mut home, &prefs);
            });
        })
        .unwrap();
    }
}
