//! Scene graph, file format, and undo/redo history. No rendering, no UI — see PLAN.md.

pub mod camera;
pub mod erase;
pub mod file_format;
pub mod fitting;
mod geometry;
pub mod history;
pub mod lasso;
pub mod scene;
pub mod text_markup;
