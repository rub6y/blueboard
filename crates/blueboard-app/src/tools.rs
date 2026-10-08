//! The drawing-tool registry — the same `name` + trait-object pattern as
//! `commands.rs`'s `CommandRegistry`, applied to what gets drawn instead of what gets
//! typed (PLAN.md's architecture section describes both as one addon-friendly shape).
//!
//! Unlike `Command`, a `Tool` never needs the whole `App` — only the camera (to convert
//! screen to board coordinates) and the scene (to add what it draws). Giving it exactly
//! that, via [`ToolContext`], rather than `&mut App`, means tool dispatch never runs into
//! the kind of whole-self borrow conflict `commands.rs`/`main.rs` had to work around.

use blueboard_core::camera::Camera;
use blueboard_core::erase;
use blueboard_core::fitting::fit_stroke;
use blueboard_core::scene::Scene;
use blueboard_render::ScenePreview;
use glam::DVec2;
use kurbo::{Affine, BezPath, Shape as _};

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
            Box::new(SelectionTool::default()),
            Box::new(EraserTool::default()),
            Box::new(EraserObjectTool::default()),
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

    /// Tool names starting with `prefix` — drives the command line's Tab-completion for
    /// `:tool <name>`'s argument, the one built-in command whose argument has a fixed,
    /// completable vocabulary.
    pub fn completions(&self, prefix: &str) -> Vec<&'static str> {
        self.tools
            .iter()
            .map(|tool| tool.name())
            .filter(|name| name.starts_with(prefix))
            .collect()
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

/// Screen-space radius (at the zoom level a click happens at, same conversion pattern as
/// [`PEN_FIT_TOLERANCE_PX`]) within which a click counts as hitting an element's outline —
/// loose enough to forgive clicking a pixel or two off a thin stroke.
const SELECT_HIT_TOLERANCE_PX: f64 = 6.0;

/// Click-to-select, drag-to-move. Unlike `ShapeTool`, a drag here never creates anything
/// — it mutates an existing element's path, and only on release: `on_pointer_move` can't
/// safely commit to the scene itself (see `main.rs`'s tool-dispatch comment — only pointer
/// down/up are snapshotted for undo), so a move is previewed from the original path and
/// applied in one step in `on_pointer_up`.
#[derive(Default)]
pub struct SelectionTool {
    selected: Option<u64>,
    /// The selected element's current path, kept in sync on select and after a committed
    /// move. This exists so [`Tool::preview`] can show the selection box at rest, without
    /// needing scene access — `preview`'s signature deliberately has none, see `tools.rs`'s
    /// module doc comment on why `Tool` doesn't take `&mut App`.
    selected_path: Option<BezPath>,
    /// Set from pointer-down until release: the selected element's path *as it was when
    /// grabbed*, and the board-space point it was grabbed at. `on_pointer_move` always
    /// translates from this fixed original rather than compounding translations onto an
    /// already-moved path, so floating-point drift can't accumulate over a long drag.
    drag: Option<(BezPath, DVec2)>,
    /// The in-progress dragged path, recomputed each `on_pointer_move`; `None` while not
    /// dragging, distinct from `selected_path` being `Some` but at rest.
    dragging_path: Option<BezPath>,
}

impl Tool for SelectionTool {
    fn name(&self) -> &'static str {
        "select"
    }

    fn on_pointer_down(&mut self, ctx: &mut ToolContext, screen_pos: DVec2) {
        let board = ctx.screen_to_board(screen_pos);
        let tolerance = SELECT_HIT_TOLERANCE_PX / ctx.camera.zoom;
        self.selected = ctx.scene.hit_test(board, tolerance);
        self.selected_path = self
            .selected
            .and_then(|id| ctx.scene.element(id))
            .map(|element| element.path.clone());
        self.drag = self.selected_path.clone().map(|path| (path, board));
        self.dragging_path = None;
    }

    fn on_pointer_move(&mut self, ctx: &mut ToolContext, screen_pos: DVec2) {
        let Some((original, grabbed_at)) = &self.drag else {
            return;
        };
        let delta = ctx.screen_to_board(screen_pos) - *grabbed_at;
        self.dragging_path = Some(Affine::translate((delta.x, delta.y)) * original);
    }

    fn on_pointer_up(&mut self, ctx: &mut ToolContext, _screen_pos: DVec2) {
        if let (Some(id), Some(path)) = (self.selected, self.dragging_path.take()) {
            ctx.scene.set_element_path(id, path.clone());
            self.selected_path = Some(path);
        }
        self.drag = None;
    }

    fn preview(&self) -> ScenePreview<'_> {
        match self.dragging_path.as_ref().or(self.selected_path.as_ref()) {
            Some(path) => ScenePreview::Selection(path.bounding_box()),
            None => ScenePreview::None,
        }
    }
}

/// Screen-space radius (converted to board-space the same way as every other tolerance in
/// this file) of the partial eraser's circle — wide enough to feel like an actual eraser
/// rather than a pin-point, narrow enough not to take out unrelated nearby strokes.
const ERASER_RADIUS_PX: f64 = 10.0;

/// Partial eraser: subtracts a circular region from whatever strokes/shapes it passes over
/// (`blueboard_core::erase`), splitting them rather than deleting them whole. Mutates the
/// scene directly on every `on_pointer_down`/`on_pointer_move` while dragging — unlike the
/// selection tool, there's no "preview then commit" step to get right, since erasing has no
/// ambiguous in-progress state to roll back if the drag were abandoned (it can't be; there's
/// no cancel gesture for a drag already in flight). `main.rs` still only snapshots once
/// around the whole pointer-down..up span, so a multi-element erase drag collapses to one
/// undo step, same as `SelectionTool`'s move.
#[derive(Default)]
pub struct EraserTool {
    erasing: bool,
    /// Last-known board-space cursor position, kept even while not erasing so `preview` has
    /// something to draw the moment the tool becomes active (before any drag starts).
    cursor: DVec2,
    radius: f64,
}

impl EraserTool {
    fn update_cursor(&mut self, ctx: &ToolContext, screen_pos: DVec2) {
        self.cursor = ctx.screen_to_board(screen_pos);
        self.radius = ERASER_RADIUS_PX / ctx.camera.zoom;
    }
}

impl Tool for EraserTool {
    fn name(&self) -> &'static str {
        "eraser"
    }

    fn on_pointer_down(&mut self, ctx: &mut ToolContext, screen_pos: DVec2) {
        self.erasing = true;
        self.update_cursor(ctx, screen_pos);
        erase::erase_circle(ctx.scene, self.cursor, self.radius);
    }

    fn on_pointer_move(&mut self, ctx: &mut ToolContext, screen_pos: DVec2) {
        self.update_cursor(ctx, screen_pos);
        if self.erasing {
            erase::erase_circle(ctx.scene, self.cursor, self.radius);
        }
    }

    fn on_pointer_up(&mut self, _ctx: &mut ToolContext, _screen_pos: DVec2) {
        self.erasing = false;
    }

    fn preview(&self) -> ScenePreview<'_> {
        ScenePreview::EraseCursor {
            center: self.cursor,
            radius: self.radius,
        }
    }
}

/// Whole-object eraser: removes entire elements it passes over, via the same
/// `hit_test`-by-outline `SelectionTool` uses to pick a click target, except every
/// candidate it touches is deleted rather than just the nearest one selected.
#[derive(Default)]
pub struct EraserObjectTool {
    erasing: bool,
    cursor: DVec2,
    radius: f64,
}

impl Tool for EraserObjectTool {
    fn name(&self) -> &'static str {
        "eraser-object"
    }

    fn on_pointer_down(&mut self, ctx: &mut ToolContext, screen_pos: DVec2) {
        self.erasing = true;
        self.erase_at(ctx, screen_pos);
    }

    fn on_pointer_move(&mut self, ctx: &mut ToolContext, screen_pos: DVec2) {
        self.cursor = ctx.screen_to_board(screen_pos);
        self.radius = SELECT_HIT_TOLERANCE_PX / ctx.camera.zoom;
        if self.erasing {
            self.erase_at(ctx, screen_pos);
        }
    }

    fn on_pointer_up(&mut self, _ctx: &mut ToolContext, _screen_pos: DVec2) {
        self.erasing = false;
    }

    fn preview(&self) -> ScenePreview<'_> {
        ScenePreview::EraseCursor {
            center: self.cursor,
            radius: self.radius,
        }
    }
}

impl EraserObjectTool {
    fn erase_at(&mut self, ctx: &mut ToolContext, screen_pos: DVec2) {
        self.cursor = ctx.screen_to_board(screen_pos);
        self.radius = SELECT_HIT_TOLERANCE_PX / ctx.camera.zoom;
        if let Some(id) = ctx.scene.hit_test(self.cursor, self.radius) {
            ctx.scene.remove_element(id);
        }
    }
}
