//! Undo/redo history for the scene — PLAN.md Phase 5's "the `core` history stack".
//!
//! The design is *snapshot*-based (the memento pattern): recording an edit stores a copy
//! of the scene as it was *before* the edit, and undo restores one of those copies while
//! pushing the current scene onto the redo stack. The alternative — storing edits as
//! inverse-operation pairs (the classic command pattern: `add`/`remove`,
//! `clear`/`repopulate`, ...) — was rejected for now because every new kind of edit
//! (eraser, selection move, text — Phase 5's remaining items) would need its own
//! hand-written inverse, and an inverse that's wrong in an edge case silently corrupts a
//! board, whereas a snapshot can't disagree with the state it captures. The cost is
//! memory proportional to [`HISTORY_LIMIT`] × board size, which is bounded; if it ever
//! matters, the storage behind this module's API is the only thing that changes.
//!
//! What is *not* recorded: the camera (pan/zoom is navigation, not editing), the active
//! tool, and a stroke's in-progress preview — only committed scene edits passed through
//! [`History::record`].

use std::collections::VecDeque;

use crate::scene::Scene;

/// How many edits the undo stack keeps. The oldest edit silently falls off once this is
/// exceeded — a deliberate cap so an hours-long session can't grow the stack without
/// bound; 100 is far past what a single sitting usually needs to walk back through.
const HISTORY_LIMIT: usize = 100;

/// A copy of a scene's elements: `(id, path, color)` triples sorted by id.
///
/// Opaque to everything outside this crate's `scene`/`history` modules — callers pass it
/// between [`Scene::snapshot`] and [`History`] without looking inside.
#[derive(Clone, PartialEq)]
pub struct Snapshot {
    pub(crate) elements: Vec<(u64, kurbo::BezPath, [f32; 3])>,
}

/// The undo and redo stacks for one scene. Lives on the app's `App` struct (see
/// `blueboard-app/src/main.rs`); empty history means "nothing to undo/redo", not an
/// error state.
#[derive(Default)]
pub struct History {
    /// Snapshots of the scene *as it was before* each recorded edit, oldest first —
    /// undo pops from the back, so the most recent edit is undone first.
    undo: VecDeque<Snapshot>,
    /// Snapshots that a redo would restore back into existence. Only populated between
    /// an undo and the next recorded edit; any new edit clears it (it no longer matches
    /// what actually happened next).
    redo: VecDeque<Snapshot>,
}

impl History {
    /// Records `before` — a snapshot taken before an operation ran — as an undo point
    /// *if* the operation actually changed the scene. Recording the unchanged case
    /// would put a no-op on the stack, making undo appear broken ("undo" does nothing,
    /// then the edit after it is one press further away than it looks).
    pub fn record(&mut self, before: Snapshot, scene: &Scene) {
        if before == scene.snapshot() {
            return;
        }
        if self.undo.len() >= HISTORY_LIMIT {
            self.undo.pop_front();
        }
        self.undo.push_back(before);
        self.redo.clear();
    }

    /// Restores the scene to its state before the most recent recorded edit. Returns
    /// `false` (leaving the scene untouched) if there is nothing to undo.
    pub fn undo(&mut self, scene: &mut Scene) -> bool {
        let Some(before) = self.undo.pop_back() else {
            return false;
        };
        self.redo.push_back(scene.snapshot());
        scene.restore(before);
        true
    }

    /// Re-applies the most recently undone edit. Returns `false` (leaving the scene
    /// untouched) if there is nothing to redo.
    pub fn redo(&mut self, scene: &mut Scene) -> bool {
        let Some(next) = self.redo.pop_back() else {
            return false;
        };
        self.undo.push_back(scene.snapshot());
        scene.restore(next);
        true
    }
}

#[cfg(test)]
mod tests {
    use kurbo::BezPath;

    use super::*;

    fn sample_path(offset: f64) -> BezPath {
        let mut path = BezPath::new();
        path.move_to((offset, 0.0));
        path.curve_to(
            (offset + 1.0, 1.0),
            (offset + 2.0, 2.0),
            (offset + 3.0, 3.0),
        );
        path
    }

    /// Records one committed edit: `add_element`, with the snapshot taken beforehand —
    /// the same sequence the app performs around tool dispatch and commands.
    fn add_and_record(history: &mut History, scene: &mut Scene, offset: f64) {
        let before = scene.snapshot();
        scene.add_element(sample_path(offset));
        history.record(before, scene);
    }

    #[test]
    fn undo_restores_scene_to_before_the_edit() {
        let mut history = History::default();
        let mut scene = Scene::new();
        add_and_record(&mut history, &mut scene, 0.0);
        assert_eq!(scene.len(), 1);

        assert!(history.undo(&mut scene));
        assert!(scene.is_empty());
    }

    #[test]
    fn undo_with_no_history_is_a_no_op() {
        let mut history = History::default();
        let mut scene = Scene::new();
        scene.add_element(sample_path(0.0));

        assert!(!history.undo(&mut scene));
        assert_eq!(scene.len(), 1);
    }

    #[test]
    fn redo_reapplies_an_undone_edit() {
        let mut history = History::default();
        let mut scene = Scene::new();
        add_and_record(&mut history, &mut scene, 0.0);
        history.undo(&mut scene);
        assert!(scene.is_empty());

        assert!(history.redo(&mut scene));
        assert_eq!(scene.len(), 1);
        assert!(!history.redo(&mut scene));
    }

    #[test]
    fn a_new_edit_after_undo_clears_the_redo_stack() {
        let mut history = History::default();
        let mut scene = Scene::new();
        add_and_record(&mut history, &mut scene, 0.0);
        add_and_record(&mut history, &mut scene, 10.0);
        history.undo(&mut scene);
        assert_eq!(scene.len(), 1);

        add_and_record(&mut history, &mut scene, 20.0);
        assert!(!history.redo(&mut scene));
    }

    #[test]
    fn an_operation_that_changed_nothing_records_nothing() {
        let mut history = History::default();
        let mut scene = Scene::new();
        let before = scene.snapshot();
        history.record(before, &scene);

        assert!(!history.undo(&mut scene));
    }

    #[test]
    fn history_limit_drops_the_oldest_edit() {
        let mut history = History::default();
        let mut scene = Scene::new();
        for i in 0..(HISTORY_LIMIT + 5) {
            add_and_record(&mut history, &mut scene, i as f64);
        }

        let mut undos = 0;
        while history.undo(&mut scene) {
            undos += 1;
        }
        assert_eq!(undos, HISTORY_LIMIT);
        // The five oldest edits' snapshots fell off; the scene stops where the stack
        // runs out.
        assert_eq!(scene.len(), 5);
    }

    #[test]
    fn undo_restores_a_cleared_scene_without_reusing_ids() {
        let mut history = History::default();
        let mut scene = Scene::new();
        scene.add_element(sample_path(0.0)); // id 0
        scene.add_element(sample_path(10.0)); // id 1

        let before = scene.snapshot();
        scene = Scene::new(); // what `:clear`/`:new` do
        history.record(before, &scene);
        assert!(scene.is_empty());

        assert!(history.undo(&mut scene));
        assert_eq!(scene.len(), 2);
        // Ids must continue past the restored ones, even though the cleared scene's
        // `next_id` was back to 0 — a reused id would collide with a restored element.
        let id = scene.add_element(sample_path(20.0));
        assert_eq!(id, 2);
    }

    #[test]
    fn undo_walks_back_multiple_edits_in_reverse_order() {
        let mut history = History::default();
        let mut scene = Scene::new();
        for i in 0..3 {
            add_and_record(&mut history, &mut scene, i as f64);
        }

        assert!(history.undo(&mut scene));
        assert_eq!(scene.len(), 2);
        assert!(history.undo(&mut scene));
        assert_eq!(scene.len(), 1);
        assert!(history.undo(&mut scene));
        assert!(scene.is_empty());
        assert!(!history.undo(&mut scene));
    }
}
