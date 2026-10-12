//! Generates a deliberately large synthetic board and saves it to a `.bbd` file — for
//! Phase 6's "profile against a deliberately large synthetic board" (PLAN.md), so there's
//! something repeatable to measure against rather than hand-drawing thousands of strokes.
//!
//! Usage: `cargo run -p blueboard-core --example gen_stress_board -- <path> [count]`
//! (count defaults to 5000).

use blueboard_core::config::Config;
use blueboard_core::file_format;
use blueboard_core::scene::Scene;
use kurbo::BezPath;

/// A cheap deterministic PRNG (xorshift) — good enough for scattering test geometry, and
/// deterministic so a run is reproducible without pulling in a `rand` dependency just for
/// a one-off example.
struct Rng(u64);
impl Rng {
    fn next_f64(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// A short multi-segment zigzag path, standing in for a pen stroke — close enough in
/// vertex/tessellation cost to what a real board's strokes look like, without needing
/// `fitting::fit_stroke`'s curve-fit machinery here.
fn random_stroke(rng: &mut Rng, cx: f64, cy: f64) -> BezPath {
    let mut path = BezPath::new();
    path.move_to((cx, cy));
    for _ in 0..6 {
        let dx = (rng.next_f64() - 0.5) * 40.0;
        let dy = (rng.next_f64() - 0.5) * 40.0;
        path.line_to((cx + dx, cy + dy));
    }
    path
}

fn random_color(rng: &mut Rng) -> [f32; 3] {
    [
        rng.next_f64() as f32,
        rng.next_f64() as f32,
        rng.next_f64() as f32,
    ]
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "stress.bbd".to_string());
    let count: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(5000);

    // Scattered over a wide area (not clustered at the origin) so panning around the
    // board is itself part of what gets profiled, not just one dense spot.
    let extent = (count as f64).sqrt() * 60.0;

    let mut rng = Rng(0x9E3779B97F4A7C15);
    let mut scene = Scene::new();
    for _ in 0..count {
        let cx = (rng.next_f64() - 0.5) * extent;
        let cy = (rng.next_f64() - 0.5) * extent;
        scene.add_element_with_color(random_stroke(&mut rng, cx, cy), random_color(&mut rng));
    }

    file_format::save(&scene, &Config::default(), &path).expect("save should succeed");
    println!("wrote {count} elements to {path} (board extent ~{extent:.0} units)");
}
