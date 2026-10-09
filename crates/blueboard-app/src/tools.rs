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
use blueboard_core::lasso;
use blueboard_core::resize::{self, ResizeHandle};
use blueboard_core::scene::{ElementKind, Scene};
use blueboard_render::ScenePreview;
use glam::DVec2;
use kurbo::{Affine, BezPath, Point, Shape as _};

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
    /// The color whatever's drawn next should use — `App::current_color`, set by
    /// `:color`. A tool that commits an element reads this at `on_pointer_down` (not
    /// `on_pointer_up`) and holds onto it in its own state for the rest of the gesture,
    /// since `:color` could change mid-drag and `Tool::preview` has no `ctx` to re-read it
    /// from — see `PenTool`/`ShapeTool`'s own `color` fields.
    pub color: [f32; 3],
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

    /// Removes whatever this tool currently has selected from `scene`, if anything, and
    /// clears its own selection state — returns whether anything was actually removed, so
    /// `DeleteCommand` (`commands.rs`) knows whether to report "nothing selected" instead
    /// of silently doing nothing. Most tools have no notion of a standing selection and
    /// no-op here; the selection-based tools (`select`, `select-rect`, `select-lasso`)
    /// override it.
    fn delete_selection(&mut self, _scene: &mut Scene) -> bool {
        false
    }

    /// The single element this tool currently has selected, if any and if it only ever
    /// has one (the group-select tools have *a* selection but not *a single element*, so
    /// they stay at the `None` default) — used by `main.rs`'s Enter-to-edit-text gesture
    /// to find what to re-open, without needing a general multi-element query no other
    /// caller needs yet.
    fn selected_element(&self) -> Option<u64> {
        None
    }

    /// Every element this tool currently has selected — unlike [`Tool::selected_element`],
    /// covers the group-select tools too, for `:yank` (`commands.rs`) to copy whatever's
    /// selected regardless of which selection tool is active. Empty default; overridden by
    /// `select`/`select-rect`/`select-lasso`.
    fn selected_elements(&self) -> Vec<u64> {
        Vec::new()
    }

    /// Replaces whatever this tool currently has selected with `ids` — used by
    /// `PasteCommand` (`commands.rs`) to select the freshly pasted elements, so they're
    /// immediately draggable without an extra click. No-op default; overridden by
    /// `select`/`select-rect`/`select-lasso`, the same tools [`Tool::selected_elements`]
    /// covers.
    fn select_elements(&mut self, _scene: &Scene, _ids: &[u64]) {}
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
            Box::new(TextTool),
            Box::new(SelectionTool::default()),
            Box::new(EraserTool::default()),
            Box::new(EraserObjectTool::default()),
            Box::new(SelectRectTool::default()),
            Box::new(SelectLassoTool::default()),
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
    /// The color this stroke is being drawn in — read from `ctx.color` at pointer-down
    /// and held here for the rest of the gesture, since `preview` has no `ctx` to read it
    /// from directly (see `ToolContext::color`'s doc comment).
    color: [f32; 3],
}

impl Tool for PenTool {
    fn name(&self) -> &'static str {
        "pen"
    }

    fn on_pointer_down(&mut self, ctx: &mut ToolContext, screen_pos: DVec2) {
        self.drawing = true;
        self.color = ctx.color;
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
                let id = ctx.scene.add_element_with_color(path, self.color);
                log::info!("stroke {id} committed from {raw_points} raw samples");
            } else {
                log::warn!("fit_stroke returned None for {raw_points} raw samples");
            }
        }
        self.current_stroke.clear();
    }

    fn preview(&self) -> ScenePreview<'_> {
        if self.drawing && self.current_stroke.len() >= 2 {
            ScenePreview::RawPoints {
                points: &self.current_stroke,
                color: self.color,
            }
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
    /// The color this shape is being drawn in — same "captured at pointer-down, since
    /// `preview` has no `ctx`" reasoning as `PenTool::color`.
    color: [f32; 3],
}

impl ShapeTool {
    pub fn rect() -> Self {
        Self {
            kind: ShapeKind::Rect,
            tool_name: "rect",
            start: None,
            current: None,
            color: blueboard_core::scene::DEFAULT_ELEMENT_COLOR,
        }
    }

    pub fn ellipse() -> Self {
        Self {
            kind: ShapeKind::Ellipse,
            tool_name: "ellipse",
            start: None,
            current: None,
            color: blueboard_core::scene::DEFAULT_ELEMENT_COLOR,
        }
    }

    pub fn line() -> Self {
        Self {
            kind: ShapeKind::Line,
            tool_name: "line",
            start: None,
            current: None,
            color: blueboard_core::scene::DEFAULT_ELEMENT_COLOR,
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
        self.color = ctx.color;
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
            ctx.scene
                .add_element_with_color(self.build_path(start, board), self.color);
        }
        self.current = None;
    }

    fn preview(&self) -> ScenePreview<'_> {
        match &self.current {
            Some(path) => ScenePreview::Path {
                path,
                color: self.color,
            },
            None => ScenePreview::None,
        }
    }
}

/// Screen-space radius (at the zoom level a click happens at, same conversion pattern as
/// [`PEN_FIT_TOLERANCE_PX`]) within which a click counts as hitting an element's outline —
/// used by the whole-object eraser's hit-test, where a tight tolerance matters more (not
/// accidentally erasing a neighboring element) than forgiveness. See
/// [`SELECTION_CLICK_TOLERANCE_PX`] for the selection tools' own, deliberately looser,
/// version of this.
const SELECT_HIT_TOLERANCE_PX: f64 = 6.0;

/// Like [`SELECT_HIT_TOLERANCE_PX`], but for the selection tools' click-to-select and
/// resize-handle hit-testing — deliberately larger, since a selection click needs to be
/// easy to land rather than needing to land precisely on a thin outline.
const SELECTION_CLICK_TOLERANCE_PX: f64 = 14.0;

/// Captured at the pointer-down that started a resize gesture — everything needed to
/// recompute the dragged path/font size fresh from the original each `on_pointer_move`,
/// same anti-drift reasoning as `SelectionTool::drag` for a move.
struct ResizeDrag {
    handle: ResizeHandle,
    original_path: BezPath,
    original_bbox: kurbo::Rect,
    /// `Some(font_size)` if the selected element is `ElementKind::Text` — resizing scales
    /// it by the drag's geometric-mean scale factor alongside the bounding box; `None` for
    /// every other kind, which only resizes geometrically.
    original_font_size: Option<f32>,
}

/// Click-to-select, drag-to-move. Unlike `ShapeTool`, a drag here never creates anything —
/// it mutates an existing element's path, and only on release: `on_pointer_move` can't
/// safely commit to the scene itself (see `main.rs`'s tool-dispatch comment — only pointer
/// down/up are snapshotted for undo), so a move is previewed from the original path and
/// applied in one step in `on_pointer_up`.
#[derive(Default)]
pub struct SelectionTool {
    selected: Option<u64>,
    /// The selected element's current path, kept in sync on select and after a committed
    /// move/resize. This exists so [`Tool::preview`] can show the selection box at rest,
    /// without needing scene access — `preview`'s signature deliberately has none, see
    /// `tools.rs`'s module doc comment on why `Tool` doesn't take `&mut App`.
    selected_path: Option<BezPath>,
    /// Set from pointer-down until release: the selected element's path *as it was when
    /// grabbed*, and the board-space point it was grabbed at. `on_pointer_move` always
    /// translates from this fixed original rather than compounding translations onto an
    /// already-moved path, so floating-point drift can't accumulate over a long drag.
    drag: Option<(BezPath, DVec2)>,
    /// The in-progress dragged path, recomputed each `on_pointer_move`; `None` while not
    /// dragging, distinct from `selected_path` being `Some` but at rest.
    dragging_path: Option<BezPath>,
    /// `Some` instead of `drag` while a resize-handle drag is in progress (the two are
    /// mutually exclusive — a pointer-down either grabs a handle or doesn't).
    resize: Option<ResizeDrag>,
    /// The in-progress resized font size (only set when `resize.original_font_size` is
    /// `Some`) — mirrors `dragging_path`'s "recomputed fresh each move, committed on up"
    /// shape.
    dragging_font_size: Option<f32>,
}

impl Tool for SelectionTool {
    fn name(&self) -> &'static str {
        "select"
    }

    fn on_pointer_down(&mut self, ctx: &mut ToolContext, screen_pos: DVec2) {
        let board = ctx.screen_to_board(screen_pos);
        let tolerance = SELECTION_CLICK_TOLERANCE_PX / ctx.camera.zoom;

        // A handle on the *already*-selected element takes priority over everything below
        // — clicking a handle resizes, it never re-selects/deselects/moves.
        if let Some(path) = &self.selected_path {
            let bbox = path.bounding_box();
            let handle = resize::handle_points(bbox)
                .into_iter()
                .find(|(_, point)| (board - DVec2::new(point.x, point.y)).length() <= tolerance);
            if let Some((handle, _)) = handle {
                let original_font_size = self
                    .selected
                    .and_then(|id| ctx.scene.element(id))
                    .and_then(|element| match &element.kind {
                        ElementKind::Text { font_size, .. } => Some(*font_size),
                        ElementKind::Stroke | ElementKind::Image(_) => None,
                    });
                self.resize = Some(ResizeDrag {
                    handle,
                    original_path: path.clone(),
                    original_bbox: bbox,
                    original_font_size,
                });
                self.drag = None;
                self.dragging_path = None;
                return;
            }
            // Not a handle — but anywhere inside the already-selected element's bounding
            // box still grabs it to move, not just a click landing exactly on its outline.
            // Makes an already-selected box an easy, forgiving drag target instead of a
            // thin line you have to re-aim at.
            if bbox.contains(Point::new(board.x, board.y)) {
                self.drag = Some((path.clone(), board));
                self.dragging_path = None;
                self.resize = None;
                self.dragging_font_size = None;
                return;
            }
        }

        self.selected = ctx.scene.hit_test(board, tolerance);
        self.selected_path = self
            .selected
            .and_then(|id| ctx.scene.element(id))
            .map(|element| element.path.clone());
        self.drag = self.selected_path.clone().map(|path| (path, board));
        self.dragging_path = None;
        self.resize = None;
        self.dragging_font_size = None;
    }

    fn on_pointer_move(&mut self, ctx: &mut ToolContext, screen_pos: DVec2) {
        let board = ctx.screen_to_board(screen_pos);
        if let Some(resize) = &self.resize {
            let new_bbox = resize::resized_rect(
                resize.handle,
                resize.original_bbox,
                Point::new(board.x, board.y),
            );
            let affine = resize::scale_affine(resize.original_bbox, new_bbox);
            self.dragging_path = Some(affine * &resize.original_path);
            self.dragging_font_size = resize.original_font_size.map(|original| {
                let factor = resize::uniform_scale_factor(resize.original_bbox, new_bbox);
                (original as f64 * factor) as f32
            });
            return;
        }
        let Some((original, grabbed_at)) = &self.drag else {
            return;
        };
        let delta = board - *grabbed_at;
        self.dragging_path = Some(Affine::translate((delta.x, delta.y)) * original);
    }

    fn on_pointer_up(&mut self, ctx: &mut ToolContext, _screen_pos: DVec2) {
        if let (Some(id), Some(resize)) = (self.selected, self.resize.take()) {
            if let Some(path) = self.dragging_path.take() {
                let kind = match (resize.original_font_size, self.dragging_font_size.take()) {
                    (Some(_), Some(font_size)) => {
                        ctx.scene
                            .element(id)
                            .and_then(|element| match &element.kind {
                                ElementKind::Text { content, .. } => Some(ElementKind::Text {
                                    content: content.clone(),
                                    font_size,
                                }),
                                ElementKind::Stroke | ElementKind::Image(_) => None,
                            })
                    }
                    _ => None,
                };
                match kind {
                    Some(kind) => ctx.scene.set_element_path_and_kind(id, path.clone(), kind),
                    None => ctx.scene.set_element_path(id, path.clone()),
                }
                self.selected_path = Some(path);
            }
            return;
        }
        if let (Some(id), Some(path)) = (self.selected, self.dragging_path.take()) {
            ctx.scene.set_element_path(id, path.clone());
            self.selected_path = Some(path);
        }
        self.drag = None;
    }

    fn preview(&self) -> ScenePreview<'_> {
        match self.dragging_path.as_ref().or(self.selected_path.as_ref()) {
            Some(path) => ScenePreview::SelectionWithHandles(path.bounding_box()),
            None => ScenePreview::None,
        }
    }

    fn delete_selection(&mut self, scene: &mut Scene) -> bool {
        let Some(id) = self.selected.take() else {
            return false;
        };
        self.selected_path = None;
        self.drag = None;
        self.dragging_path = None;
        self.resize = None;
        self.dragging_font_size = None;
        scene.remove_element(id)
    }

    fn selected_element(&self) -> Option<u64> {
        self.selected
    }

    fn selected_elements(&self) -> Vec<u64> {
        self.selected.into_iter().collect()
    }

    fn select_elements(&mut self, scene: &Scene, ids: &[u64]) {
        self.selected = ids.first().copied();
        self.selected_path = self
            .selected
            .and_then(|id| scene.element(id))
            .map(|element| element.path.clone());
        self.drag = None;
        self.dragging_path = None;
        self.resize = None;
        self.dragging_font_size = None;
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

/// Places a text element: click to start composing, type, Enter to commit, Escape to
/// cancel — all handled specially in `main.rs`'s `MouseInput`/keyboard dispatch (App-level
/// `text_input_*` fields), the same way the color picker bypasses normal tool dispatch,
/// because that composing state belongs on `App`, not reachable from `ToolContext` (see
/// this module's doc comment on why a `Tool` never gets `&mut App`). This `Tool` impl
/// exists only so `text` is a selectable name for `:tool`/keybindings/the status bar —
/// its pointer handlers are genuinely unreachable in practice, `main.rs` intercepts every
/// `MouseInput` while this is the active tool before generic dispatch ever runs.
#[derive(Default)]
pub struct TextTool;

impl Tool for TextTool {
    fn name(&self) -> &'static str {
        "text"
    }

    fn on_pointer_down(&mut self, _ctx: &mut ToolContext, _screen_pos: DVec2) {}
    fn on_pointer_move(&mut self, _ctx: &mut ToolContext, _screen_pos: DVec2) {}
    fn on_pointer_up(&mut self, _ctx: &mut ToolContext, _screen_pos: DVec2) {}

    fn preview(&self) -> ScenePreview<'_> {
        ScenePreview::None
    }
}

/// Default board-space font size for a newly placed text element (`:tool text`'s click) —
/// comparable in scale to `STROKE_WIDTH` being a few board units, 24 reads as a normal
/// body-text size at 100% zoom.
pub const DEFAULT_TEXT_FONT_SIZE: f64 = 24.0;

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

/// Shared state and behavior for a "select a group, then drag to move or delete it" tool
/// — used by both [`SelectRectTool`] and [`SelectLassoTool`], which differ only in how a
/// *fresh* selection region gets defined (a dragged rectangle vs. a freehand closed
/// loop). Pulled out so the group-move/delete logic — itself a straightforward extension
/// of [`SelectionTool`]'s single-element version — isn't duplicated between them.
#[derive(Default)]
struct GroupSelection {
    selected: Vec<u64>,
    /// Kept in sync with `selected` so `preview` can show every selected element's
    /// current bounding box without needing scene access — same reasoning
    /// `SelectionTool`'s `selected_path` field documents.
    selected_paths: Vec<(u64, BezPath)>,
    /// Set from pointer-down until release when dragging the group: the board-space point
    /// grabbed at, and every selected element's id and path as grabbed — same
    /// translate-from-the-fixed-original pattern `SelectionTool` uses, so float drift
    /// can't accumulate over a long drag.
    drag: Option<(DVec2, Vec<(u64, BezPath)>)>,
    /// The in-progress dragged paths, recomputed each `on_pointer_move`; empty while not
    /// dragging.
    dragging_paths: Vec<(u64, BezPath)>,
}

impl GroupSelection {
    /// If `screen_pos` hits an element that's already part of the selection, begins a
    /// group-move drag and returns `true` — the caller shouldn't start defining a fresh
    /// selection region this gesture. Otherwise clears the selection (a fresh region is
    /// about to be drawn) and returns `false`.
    fn begin(&mut self, ctx: &mut ToolContext, screen_pos: DVec2) -> bool {
        let board = ctx.screen_to_board(screen_pos);
        let tolerance = SELECTION_CLICK_TOLERANCE_PX / ctx.camera.zoom;
        let hit_already_selected = ctx
            .scene
            .hit_test(board, tolerance)
            .is_some_and(|id| self.selected.contains(&id));
        if hit_already_selected {
            self.drag = Some((board, self.selected_paths.clone()));
            return true;
        }
        self.selected.clear();
        self.selected_paths.clear();
        self.drag = None;
        false
    }

    fn drag_move(&mut self, ctx: &mut ToolContext, screen_pos: DVec2) {
        let Some((anchor, originals)) = &self.drag else {
            return;
        };
        let delta = ctx.screen_to_board(screen_pos) - *anchor;
        let affine = Affine::translate((delta.x, delta.y));
        self.dragging_paths = originals
            .iter()
            .map(|(id, path)| (*id, affine * path))
            .collect();
    }

    fn commit_move(&mut self, ctx: &mut ToolContext) {
        if self.drag.take().is_some() {
            let paths = std::mem::take(&mut self.dragging_paths);
            for (id, path) in &paths {
                ctx.scene.set_element_path(*id, path.clone());
            }
            self.selected_paths = paths;
        }
    }

    /// Replaces the selection with `ids`, reading each one's current path from the scene
    /// — how a freshly-drawn marquee/lasso region becomes the new selection.
    fn set_selected(&mut self, scene: &Scene, ids: Vec<u64>) {
        self.selected_paths = ids
            .iter()
            .filter_map(|&id| scene.element(id).map(|element| (id, element.path.clone())))
            .collect();
        self.selected = ids;
    }

    fn preview(&self) -> ScenePreview<'_> {
        let paths = if !self.dragging_paths.is_empty() {
            &self.dragging_paths
        } else {
            &self.selected_paths
        };
        if paths.is_empty() {
            ScenePreview::None
        } else {
            ScenePreview::MultiSelection(
                paths.iter().map(|(_, path)| path.bounding_box()).collect(),
            )
        }
    }

    fn delete(&mut self, scene: &mut Scene) -> bool {
        if self.selected.is_empty() {
            return false;
        }
        let mut changed = false;
        for id in self.selected.drain(..) {
            changed |= scene.remove_element(id);
        }
        self.selected_paths.clear();
        self.drag = None;
        self.dragging_paths.clear();
        changed
    }
}

/// Rectangle (marquee) multi-select: drag over empty space to select every element whose
/// bounding box overlaps the dragged rectangle at all (touch selection, not containment —
/// reuses `Scene::elements_in_viewport` directly, the same broad-phase query viewport
/// culling already runs). Dragging from an already-selected element instead moves the
/// whole group — see [`GroupSelection`].
#[derive(Default)]
pub struct SelectRectTool {
    group: GroupSelection,
    marquee_start: Option<DVec2>,
    marquee_current: Option<DVec2>,
}

impl Tool for SelectRectTool {
    fn name(&self) -> &'static str {
        "select-rect"
    }

    fn on_pointer_down(&mut self, ctx: &mut ToolContext, screen_pos: DVec2) {
        if self.group.begin(ctx, screen_pos) {
            return;
        }
        let board = ctx.screen_to_board(screen_pos);
        self.marquee_start = Some(board);
        self.marquee_current = Some(board);
    }

    fn on_pointer_move(&mut self, ctx: &mut ToolContext, screen_pos: DVec2) {
        if self.group.drag.is_some() {
            self.group.drag_move(ctx, screen_pos);
        } else if self.marquee_start.is_some() {
            self.marquee_current = Some(ctx.screen_to_board(screen_pos));
        }
    }

    fn on_pointer_up(&mut self, ctx: &mut ToolContext, _screen_pos: DVec2) {
        if self.group.drag.is_some() {
            self.group.commit_move(ctx);
            return;
        }
        let (Some(start), Some(current)) = (self.marquee_start.take(), self.marquee_current.take())
        else {
            return;
        };
        let min = DVec2::new(start.x.min(current.x), start.y.min(current.y));
        let max = DVec2::new(start.x.max(current.x), start.y.max(current.y));
        let ids = ctx
            .scene
            .elements_in_viewport(min, max)
            .map(|e| e.id)
            .collect();
        self.group.set_selected(ctx.scene, ids);
    }

    fn preview(&self) -> ScenePreview<'_> {
        if let (Some(start), Some(current)) = (self.marquee_start, self.marquee_current) {
            return ScenePreview::Selection(kurbo::Rect::from_points(
                (start.x, start.y),
                (current.x, current.y),
            ));
        }
        self.group.preview()
    }

    fn delete_selection(&mut self, scene: &mut Scene) -> bool {
        self.group.delete(scene)
    }

    fn selected_elements(&self) -> Vec<u64> {
        self.group.selected.clone()
    }

    fn select_elements(&mut self, scene: &Scene, ids: &[u64]) {
        self.group.set_selected(scene, ids.to_vec());
    }
}

/// Minimum screen-space distance between recorded lasso points — same decimation
/// reasoning as the pen tool's own `PEN_MIN_SAMPLE_DISTANCE_PX`, just for a selection
/// gesture instead of a permanent stroke: pointer-move events arrive far denser than the
/// loop shape actually needs, and this is a selection boundary, not ink, so there's no
/// streamline-smoothing step to match the pen's.
const LASSO_MIN_SAMPLE_DISTANCE_PX: f64 = 3.0;

/// Lasso multi-select: draw a freehand closed loop over empty space to select every
/// element the loop touches (see `blueboard_core::lasso`). Dragging from an
/// already-selected element instead moves the whole group — see [`GroupSelection`].
#[derive(Default)]
pub struct SelectLassoTool {
    group: GroupSelection,
    /// Board-space points of the loop currently being drawn; cleared once it's finalized
    /// into a selection (or abandoned by a plain click).
    loop_points: Vec<DVec2>,
    drawing: bool,
    last_sample_screen: DVec2,
}

impl Tool for SelectLassoTool {
    fn name(&self) -> &'static str {
        "select-lasso"
    }

    fn on_pointer_down(&mut self, ctx: &mut ToolContext, screen_pos: DVec2) {
        if self.group.begin(ctx, screen_pos) {
            self.drawing = false;
            self.loop_points.clear();
            return;
        }
        self.drawing = true;
        self.last_sample_screen = screen_pos;
        self.loop_points.clear();
        self.loop_points.push(ctx.screen_to_board(screen_pos));
    }

    fn on_pointer_move(&mut self, ctx: &mut ToolContext, screen_pos: DVec2) {
        if self.group.drag.is_some() {
            self.group.drag_move(ctx, screen_pos);
            return;
        }
        if !self.drawing {
            return;
        }
        if screen_pos.distance(self.last_sample_screen) >= LASSO_MIN_SAMPLE_DISTANCE_PX {
            self.loop_points.push(ctx.screen_to_board(screen_pos));
            self.last_sample_screen = screen_pos;
        }
    }

    fn on_pointer_up(&mut self, ctx: &mut ToolContext, _screen_pos: DVec2) {
        if self.group.drag.is_some() {
            self.group.commit_move(ctx);
            return;
        }
        self.drawing = false;
        let ids = lasso::elements_touching_loop(ctx.scene, &self.loop_points);
        self.group.set_selected(ctx.scene, ids);
        self.loop_points.clear();
    }

    fn preview(&self) -> ScenePreview<'_> {
        if self.drawing && self.loop_points.len() >= 2 {
            // Not board content being drawn — the loop itself never becomes an element —
            // so this ignores `ctx.color` and stays the fixed default, the same reasoning
            // `GroupSelection::preview`'s UI-only boxes already follow.
            return ScenePreview::RawPoints {
                points: &self.loop_points,
                color: blueboard_core::scene::DEFAULT_ELEMENT_COLOR,
            };
        }
        self.group.preview()
    }

    fn delete_selection(&mut self, scene: &mut Scene) -> bool {
        self.group.delete(scene)
    }

    fn selected_elements(&self) -> Vec<u64> {
        self.group.selected.clone()
    }

    fn select_elements(&mut self, scene: &Scene, ids: &[u64]) {
        self.group.set_selected(scene, ids.to_vec());
    }
}
