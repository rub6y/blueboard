//! The drawing-tool registry — the same `name` + trait-object pattern as
//! `commands.rs`'s `CommandRegistry`, applied to what gets drawn instead of what gets
//! typed (PLAN.md's architecture section describes both as one addon-friendly shape).
//!
//! Unlike `Command`, a `Tool` never needs the whole `App` — only the camera (to convert
//! screen to board coordinates) and the scene (to add what it draws). Giving it exactly
//! that, via [`ToolContext`], rather than `&mut App`, means tool dispatch never runs into
//! the kind of whole-self borrow conflict `commands.rs`/`main.rs` had to work around.

use blueboard_core::camera::Camera;
use blueboard_core::fitting::fit_stroke;
use blueboard_core::scene::Scene;
use blueboard_render::ScenePreview;
use glam::DVec2;
use kurbo::{BezPath, Shape as _};

/// How far (in screen pixels, at the zoom level the stroke was drawn at) the fitted curve
/// is allowed to deviate from the raw pointer samples — see `planning/math.tex`'s curve
/// fitting section. Converted to board-space units by dividing by the current zoom, so a
/// stroke looks equally precise regardless of zoom level when it was drawn.
///
/// Loose enough to smooth over a pixel or two of real mouse/trackpad jitter rather than
/// faithfully reproducing it as visible kinks — tightening this past a couple of pixels
/// trades smoothness for positional fidelity nobody can actually see.
const PEN_FIT_TOLERANCE_PX: f64 = 2.5;

/// Minimum screen-space distance between recorded pointer samples while drawing.
/// Pointer-move events arrive far more densely than meaningful hand motion, so most of
/// that density is jitter, not intent — decimating by distance (rather than keeping every
/// event) stops redundant near-duplicate points from ever reaching the curve fitter.
///
/// Decimation alone only thins point *density*, though — it doesn't touch the *position*
/// of the points that survive, so real jitter is still baked into each one. [`PEN_STREAMLINE_ALPHA`]
/// below is what actually addresses that.
const PEN_MIN_SAMPLE_DISTANCE_PX: f64 = 2.0;

/// Exponential-moving-average weight given to each new raw pointer position, applied
/// *before* decimation — same idea as the "streamline" parameter in
/// [perfect-freehand](https://github.com/steveruizok/perfect-freehand): pull the recorded
/// point toward a running average of recent positions instead of recording the raw,
/// jittery position directly.
const PEN_STREAMLINE_ALPHA: f64 = 0.5;

/// Tolerance (board-space) for flattening a shape tool's exact curve into the `BezPath`
/// stored in the scene. Shapes aren't fit from noisy samples like the pen, so this only
/// needs to be tight enough that an ellipse looks round — not zoom-dependent the way
/// `blueboard-render`'s tessellation tolerance is.
const SHAPE_PATH_TOLERANCE: f64 = 0.1;

/// What a [`Tool`] needs to do its job: convert screen to board coordinates, and add to
/// the scene. Deliberately not `&mut App` — see this module's doc comment.
pub struct ToolContext<'a> {
    pub camera: &'a Camera,
    pub scene: &'a mut Scene,
    pub viewport: DVec2,
}

impl ToolContext<'_> {
    pub fn screen_to_board(&self, screen: DVec2) -> DVec2 {
        self.camera.screen_to_board(screen, self.viewport)
    }
}

pub trait Tool {
    fn name(&self) -> &'static str;

    fn on_pointer_down(&mut self, ctx: &mut ToolContext, screen_pos: DVec2);
    fn on_pointer_move(&mut self, ctx: &mut ToolContext, screen_pos: DVec2);
    fn on_pointer_up(&mut self, ctx: &mut ToolContext, screen_pos: DVec2);

    /// What to draw as a live, not-yet-committed preview right now, if anything.
    fn preview(&self) -> ScenePreview<'_>;
}

pub struct ToolRegistry {
    tools: Vec<Box<dyn Tool>>,
    active: usize,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::with_builtin_tools()
    }
}

impl ToolRegistry {
    pub fn with_builtin_tools() -> Self {
        let tools: Vec<Box<dyn Tool>> = vec![
            Box::new(PenTool::default()),
            Box::new(ShapeTool::rect()),
            Box::new(ShapeTool::ellipse()),
            Box::new(ShapeTool::line()),
        ];
        Self { tools, active: 0 }
    }

    pub fn active_tool(&self) -> &dyn Tool {
        self.tools[self.active].as_ref()
    }

    pub fn active_tool_mut(&mut self) -> &mut dyn Tool {
        self.tools[self.active].as_mut()
    }

    /// Switches the active tool by name (as used in `:tool <name>` and keybindings).
    /// Returns `false` for an unknown name, leaving the active tool unchanged.
    pub fn set_active(&mut self, name: &str) -> bool {
        match self.tools.iter().position(|tool| tool.name() == name) {
            Some(index) => {
                self.active = index;
                true
            }
            None => false,
        }
    }
}

/// Freehand drawing: raw pointer samples, smoothed and decimated, curve-fit into a
/// `BezPath` on release. The logic here is unchanged from Phases 2-4 — only *where* it
/// lives moved, from fields directly on `App` into this self-contained tool.
#[derive(Default)]
pub struct PenTool {
    drawing: bool,
    /// Board-space points of the stroke currently being drawn, in order.
    current_stroke: Vec<DVec2>,
    /// Screen-space position of the last *recorded* sample (not necessarily the last
    /// pointer-move event — see [`PEN_MIN_SAMPLE_DISTANCE_PX`]).
    last_sample_screen: DVec2,
    /// Running exponential moving average of the cursor position while drawing.
    smoothed_cursor: DVec2,
}

impl Tool for PenTool {
    fn name(&self) -> &'static str {
        "pen"
    }

    fn on_pointer_down(&mut self, ctx: &mut ToolContext, screen_pos: DVec2) {
        self.drawing = true;
        self.smoothed_cursor = screen_pos;
        self.last_sample_screen = screen_pos;
        self.current_stroke.clear();
        self.current_stroke.push(ctx.screen_to_board(screen_pos));
    }

    fn on_pointer_move(&mut self, ctx: &mut ToolContext, screen_pos: DVec2) {
        if !self.drawing {
            return;
        }
        self.smoothed_cursor = self.smoothed_cursor.lerp(screen_pos, PEN_STREAMLINE_ALPHA);
        if self.smoothed_cursor.distance(self.last_sample_screen) >= PEN_MIN_SAMPLE_DISTANCE_PX {
            self.current_stroke
                .push(ctx.screen_to_board(self.smoothed_cursor));
            self.last_sample_screen = self.smoothed_cursor;
        }
    }

    fn on_pointer_up(&mut self, ctx: &mut ToolContext, _screen_pos: DVec2) {
        self.drawing = false;
        if self.current_stroke.len() >= 2 {
            let accuracy = PEN_FIT_TOLERANCE_PX / ctx.camera.zoom;
            let raw_points = self.current_stroke.len();
            if let Some(path) = fit_stroke(&self.current_stroke, accuracy) {
                let id = ctx.scene.add_element(path);
                log::info!("stroke {id} committed from {raw_points} raw samples");
            } else {
                log::warn!("fit_stroke returned None for {raw_points} raw samples");
            }
        }
        self.current_stroke.clear();
    }

    fn preview(&self) -> ScenePreview<'_> {
        if self.drawing && self.current_stroke.len() >= 2 {
            ScenePreview::RawPoints(&self.current_stroke)
        } else {
            ScenePreview::None
        }
    }
}

enum ShapeKind {
    Rect,
    Ellipse,
    Line,
}

/// Rectangle/ellipse/line: drag from one board-space corner to another. Unlike the pen,
/// there's no fitting step — the dragged shape already *is* the exact path that gets
/// committed, so the live preview and the final element are built the same way.
pub struct ShapeTool {
    kind: ShapeKind,
    tool_name: &'static str,
    start: Option<DVec2>,
    current: Option<BezPath>,
}

impl ShapeTool {
    pub fn rect() -> Self {
        Self {
            kind: ShapeKind::Rect,
            tool_name: "rect",
            start: None,
            current: None,
        }
    }

    pub fn ellipse() -> Self {
        Self {
            kind: ShapeKind::Ellipse,
            tool_name: "ellipse",
            start: None,
            current: None,
        }
    }

    pub fn line() -> Self {
        Self {
            kind: ShapeKind::Line,
            tool_name: "line",
            start: None,
            current: None,
        }
    }

    fn build_path(&self, start: DVec2, end: DVec2) -> BezPath {
        match self.kind {
            ShapeKind::Rect => kurbo::Rect::from_points((start.x, start.y), (end.x, end.y))
                .to_path(SHAPE_PATH_TOLERANCE),
            ShapeKind::Ellipse => {
                let bounds = kurbo::Rect::from_points((start.x, start.y), (end.x, end.y));
                kurbo::Ellipse::from_rect(bounds).to_path(SHAPE_PATH_TOLERANCE)
            }
            ShapeKind::Line => {
                let mut path = BezPath::new();
                path.move_to((start.x, start.y));
                path.line_to((end.x, end.y));
                path
            }
        }
    }
}

impl Tool for ShapeTool {
    fn name(&self) -> &'static str {
        self.tool_name
    }

    fn on_pointer_down(&mut self, ctx: &mut ToolContext, screen_pos: DVec2) {
        let board = ctx.screen_to_board(screen_pos);
        self.start = Some(board);
        self.current = Some(self.build_path(board, board));
    }

    fn on_pointer_move(&mut self, ctx: &mut ToolContext, screen_pos: DVec2) {
        let Some(start) = self.start else { return };
        let board = ctx.screen_to_board(screen_pos);
        self.current = Some(self.build_path(start, board));
    }

    fn on_pointer_up(&mut self, ctx: &mut ToolContext, screen_pos: DVec2) {
        let Some(start) = self.start.take() else {
            return;
        };
        let board = ctx.screen_to_board(screen_pos);
        // Skip a degenerate shape from a click with no drag (start == end) — checked as
        // distance, not bounding-box area, since a perfectly horizontal/vertical *line*
        // legitimately has zero height/width and shouldn't be rejected for it.
        if (board - start).length() > f64::EPSILON {
            ctx.scene.add_element(self.build_path(start, board));
        }
        self.current = None;
    }

    fn preview(&self) -> ScenePreview<'_> {
        match &self.current {
            Some(path) => ScenePreview::Path(path),
            None => ScenePreview::None,
        }
    }
}
