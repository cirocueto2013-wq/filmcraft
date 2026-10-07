//! Bounded bidirectional block flow for motion-compensated retiming. Estimate displacement on
//! a reduced image, interpolate the vector grid, then warp the original linear-light frames.
//! Flat regions prefer zero motion; unreliable matches (including cuts) fall back to blending.

use rayon::prelude::*;

use crate::Image;

const STEP: usize = 8;
const SEARCH: isize = 8;
const PATCH: isize = 3;

struct Flow {
    width: usize,
    height: usize,
    vectors: Vec<[f32; 2]>,
}

fn reduced(image: &Image) -> Image {
    let mut small = if image.w.max(image.h) > 128 { image.downsample2() } else { image.clone() };
    while small.w.max(small.h) > 128 {
        small = small.downsample2();
    }
    small
}

fn estimate(a: &Image, b: &Image) -> Flow {
    let width = a.w.div_ceil(STEP);
    let height = a.h.div_ceil(STEP);
    let mut vectors = vec![[0.0; 2]; width * height];
    for gy in 0..height {
        if filmcraft_media::cancel::cancelled() {
            break;
        }
        for gx in 0..width {
            let x = (gx * STEP + STEP / 2).min(a.w - 1) as isize;
            let y = (gy * STEP + STEP / 2).min(a.h - 1) as isize;
            let mut best = f32::INFINITY;
            let mut vector = [0.0; 2];
            for dy in -SEARCH..=SEARCH {
                for dx in -SEARCH..=SEARCH {
                    if x + dx < 0 || y + dy < 0 || x + dx >= b.w as isize || y + dy >= b.h as isize {
                        continue;
                    }
                    let mut error = 0.0;
                    for py in -PATCH..=PATCH {
                        for px in -PATCH..=PATCH {
                            let p = a.get_clamped(x + px, y + py);
                            let q = b.get_clamped(x + px + dx, y + py + dy);
                            for channel in 0..4 {
                                error += (p[channel] - q[channel]).abs();
                            }
                        }
                    }
                    error /= ((PATCH * 2 + 1).pow(2) * 4) as f32;
                    // A textureless patch has many equally good matches. Prefer the shortest vector.
                    let score = error + (dx * dx + dy * dy) as f32 * 0.00001;
                    if score < best {
                        best = score;
                        vector = [dx as f32, dy as f32];
                    }
                }
            }
            if best < 0.15 {
                vectors[gy * width + gx] = vector;
            }
        }
    }
    Flow { width, height, vectors }
}

impl Flow {
    fn at(&self, x: f32, y: f32) -> [f32; 2] {
        let x = (x / STEP as f32 - 0.5).clamp(0.0, self.width.saturating_sub(1) as f32);
        let y = (y / STEP as f32 - 0.5).clamp(0.0, self.height.saturating_sub(1) as f32);
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(self.width - 1), (y0 + 1).min(self.height - 1));
        let fetch = |x, y| self.vectors.get(y * self.width + x).copied().unwrap_or([0.0; 2]);
        let (a, b, c, d) = (fetch(x0, y0), fetch(x1, y0), fetch(x0, y1), fetch(x1, y1));
        let (tx, ty) = (x - x0 as f32, y - y0 as f32);
        std::array::from_fn(|k| {
            let top = a[k] + (b[k] - a[k]) * tx;
            let bottom = c[k] + (d[k] - c[k]) * tx;
            top + (bottom - top) * ty
        })
    }
}

/// Interpolate two consecutive decoded frames, preserving the original resolution and alpha.
pub fn interpolate(a: Image, b: &Image, weight: f32) -> Image {
    let weight = if weight.is_finite() { weight.clamp(0.0, 1.0) } else { 0.0 };
    if weight == 0.0 {
        return a;
    }
    if weight == 1.0 {
        return b.clone();
    }
    let count = a.w.checked_mul(a.h).and_then(|n| n.checked_mul(4));
    if a.w == 0 || a.h == 0 || a.w != b.w || a.h != b.h || count != Some(a.px.len()) || count != Some(b.px.len()) {
        return a;
    }
    let small_a = reduced(&a);
    let small_b = reduced(b);
    let forward = estimate(&small_a, &small_b);
    let backward = estimate(&small_b, &small_a);
    let sx = a.w as f32 / small_a.w as f32;
    let sy = a.h as f32 / small_a.h as f32;
    let mut output = Image::new(a.w, a.h);
    output.px.par_chunks_exact_mut(a.w * 4).enumerate().for_each(|(y, row)| {
        if filmcraft_media::cancel::cancelled() {
            return;
        }
        for (x, pixel) in row.chunks_exact_mut(4).enumerate() {
            let (cx, cy) = (x as f32 + 0.5, y as f32 + 0.5);
            let f = forward.at(cx / sx, cy / sy);
            let r = backward.at(cx / sx, cy / sy);
            let first = a.sample_bilinear_clamped(cx - weight * f[0] * sx, cy - weight * f[1] * sy);
            let second = b.sample_bilinear_clamped(cx - (1.0 - weight) * r[0] * sx, cy - (1.0 - weight) * r[1] * sy);
            for k in 0..4 {
                pixel[k] = first[k] * (1.0 - weight) + second[k] * weight;
            }
        }
    });
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texture() -> Image {
        let mut image = Image::filled(64, 48, [0.0, 0.0, 0.0, 1.0]);
        let mut seed = 13_u32;
        for pixel in image.px.chunks_exact_mut(4) {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            pixel[..3].fill((seed >> 24) as f32 / 255.0);
        }
        image
    }

    #[test]
    fn a_translated_texture_moves_instead_of_ghosting() {
        let first = texture();
        let mut later = first.clone();
        for y in 0..first.h {
            for x in 0..first.w {
                let p = first.get_clamped(x as isize - 4, y as isize);
                let offset = (y * first.w + x) * 4;
                later.px[offset..offset + 4].copy_from_slice(&p);
            }
        }
        for weight in [0.25, 0.5, 0.75] {
            let flow = interpolate(first.clone(), &later, weight);
            let blend = first.clone().lerp(&later, weight);
            let mut flow_error = 0.0;
            let mut blend_error = 0.0;
            for y in 12..36 {
                for x in 16..48 {
                    let expected = first.get(x - (weight * 4.0) as usize, y)[0];
                    flow_error += (flow.get(x, y)[0] - expected).abs();
                    blend_error += (blend.get(x, y)[0] - expected).abs();
                    assert_eq!(flow.get(x, y)[3], 1.0);
                }
            }
            assert!(flow_error < blend_error * 0.05, "{weight}: flow {flow_error} vs blending {blend_error}");
            assert_eq!(flow, interpolate(later.clone(), &first, 1.0 - weight), "reverse playback symmetry");
        }
    }

    #[test]
    fn static_frames_endpoints_and_scene_cuts_are_stable() {
        let image = texture();
        assert_eq!(interpolate(image.clone(), &image, 0.5), image);
        let other = Image::filled(64, 48, [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(interpolate(image.clone(), &other, 0.0), image);
        assert_eq!(interpolate(image.clone(), &other, 1.0), other);
        let red = Image::filled(64, 48, [1.0, 0.0, 0.0, 1.0]);
        let blue = Image::filled(64, 48, [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(interpolate(red.clone(), &blue, 0.5), red.lerp(&blue, 0.5));
        let empty = Image::new(0, 0);
        assert_eq!(interpolate(empty.clone(), &image, 0.5), empty);
    }
}
