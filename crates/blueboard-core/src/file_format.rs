//! The `.bbd` save format: [`Scene`] -> `serde` -> `bincode` -> `zstd`, per
//! `RESEARCH.md` §3. A single compressed blob for now — spatial tiling for lazy-loading
//! large boards is deferred to PLAN.md's Phase 6, once a real board exists to profile
//! against.
//!
//! The on-disk shape ([`ElementDocument`]/[`SceneDocument`]) is deliberately a separate
//! type from [`Scene`]/[`Element`], not a direct serialization of the runtime structures —
//! `Scene` also carries the R-tree index, which is derived data that shouldn't be part of
//! the file (it's rebuilt on load) and would just be dead weight on disk.

use std::fmt;
use std::path::Path;

use kurbo::BezPath;
use serde::{Deserialize, Serialize};

use crate::scene::Scene;

/// On-disk format version. Bump this whenever `SceneDocument`'s shape changes in a way
/// that would misread an older file; `load` refuses to read anything else.
///
/// Bumped to 2 for per-element color (`ElementDocument::color`) — a version-1 file has no
/// such field, so per this format's existing strict-version stance, it's rejected outright
/// rather than migrated (no default-color backfill code to maintain and potentially get
/// wrong).
const FORMAT_VERSION: u32 = 2;

#[derive(Serialize, Deserialize)]
struct ElementDocument {
    id: u64,
    path: BezPath,
    color: [f32; 3],
}

#[derive(Serialize, Deserialize)]
struct SceneDocument {
    version: u32,
    elements: Vec<ElementDocument>,
}

#[derive(Debug)]
pub enum SaveError {
    Encode(bincode::error::EncodeError),
    Io(std::io::Error),
}

impl fmt::Display for SaveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SaveError::Encode(err) => write!(f, "failed to encode scene: {err}"),
            SaveError::Io(err) => write!(f, "failed to write file: {err}"),
        }
    }
}

impl std::error::Error for SaveError {}

#[derive(Debug)]
pub enum LoadError {
    Io(std::io::Error),
    Decompress(std::io::Error),
    Decode(bincode::error::DecodeError),
    UnsupportedVersion(u32),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Io(err) => write!(f, "failed to read file: {err}"),
            LoadError::Decompress(err) => write!(f, "failed to decompress file: {err}"),
            LoadError::Decode(err) => write!(f, "failed to decode scene: {err}"),
            LoadError::UnsupportedVersion(version) => {
                write!(
                    f,
                    "unsupported file format version {version} (expected {FORMAT_VERSION})"
                )
            }
        }
    }
}

impl std::error::Error for LoadError {}

/// Save every element in `scene` (not just what's currently visible) to `path`.
pub fn save(scene: &Scene, path: impl AsRef<Path>) -> Result<(), SaveError> {
    let mut elements: Vec<ElementDocument> = scene
        .all_elements()
        .map(|element| ElementDocument {
            id: element.id,
            path: element.path.clone(),
            color: element.color,
        })
        .collect();
    // Sorted by id so the same scene always serializes to the same bytes — `Scene`
    // stores elements in a `HashMap`, whose iteration order isn't stable across runs.
    elements.sort_by_key(|element| element.id);

    let document = SceneDocument {
        version: FORMAT_VERSION,
        elements,
    };

    let encoded = bincode::serde::encode_to_vec(&document, bincode::config::standard())
        .map_err(SaveError::Encode)?;
    let compressed =
        zstd::encode_all(&encoded[..], zstd::DEFAULT_COMPRESSION_LEVEL).map_err(SaveError::Io)?;
    std::fs::write(path, compressed).map_err(SaveError::Io)
}

/// Load a scene previously written by [`save`].
pub fn load(path: impl AsRef<Path>) -> Result<Scene, LoadError> {
    let compressed = std::fs::read(path).map_err(LoadError::Io)?;
    let encoded = zstd::decode_all(&compressed[..]).map_err(LoadError::Decompress)?;
    let (document, _): (SceneDocument, usize) =
        bincode::serde::decode_from_slice(&encoded, bincode::config::standard())
            .map_err(LoadError::Decode)?;

    if document.version != FORMAT_VERSION {
        return Err(LoadError::UnsupportedVersion(document.version));
    }

    let mut scene = Scene::new();
    for element in document.elements {
        scene.insert_loaded_element(element.id, element.path, element.color);
    }
    Ok(scene)
}

#[cfg(test)]
mod tests {
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

    /// A unique path per test run (not just per test) so parallel `cargo test` runs don't
    /// collide writing to the same file in the shared system temp directory.
    fn temp_bbd_path(test_name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "blueboard-test-{test_name}-{}-{:?}.bbd",
            std::process::id(),
            std::thread::current().id(),
        ))
    }

    #[test]
    fn save_then_load_round_trips_scene() {
        let mut scene = Scene::new();
        scene.add_element(sample_path(0.0));
        scene.add_element_with_color(sample_path(10.0), [0.9, 0.2, 0.2]);

        let path = temp_bbd_path("round-trips-scene");
        save(&scene, &path).expect("save should succeed");
        let loaded = load(&path).expect("load should succeed");
        std::fs::remove_file(&path).ok();

        assert_eq!(loaded.len(), scene.len());
        let mut original: Vec<_> = scene
            .all_elements()
            .map(|e| (e.id, e.path.clone(), e.color))
            .collect();
        let mut round_tripped: Vec<_> = loaded
            .all_elements()
            .map(|e| (e.id, e.path.clone(), e.color))
            .collect();
        original.sort_by_key(|(id, _, _)| *id);
        round_tripped.sort_by_key(|(id, _, _)| *id);
        assert_eq!(original, round_tripped);
    }

    #[test]
    fn loaded_scene_continues_ids_after_the_highest_loaded_id() {
        let mut scene = Scene::new();
        scene.add_element(sample_path(0.0)); // id 0
        scene.add_element(sample_path(10.0)); // id 1

        let path = temp_bbd_path("continues-ids");
        save(&scene, &path).expect("save should succeed");
        let mut loaded = load(&path).expect("load should succeed");
        std::fs::remove_file(&path).ok();

        let new_id = loaded.add_element(sample_path(20.0));
        assert_eq!(new_id, 2);
    }

    #[test]
    fn empty_scene_round_trips() {
        let scene = Scene::new();
        let path = temp_bbd_path("empty-scene");
        save(&scene, &path).expect("save should succeed");
        let loaded = load(&path).expect("load should succeed");
        std::fs::remove_file(&path).ok();

        assert!(loaded.is_empty());
    }

    #[test]
    fn rejects_unsupported_version() {
        let document = SceneDocument {
            version: FORMAT_VERSION + 1,
            elements: vec![],
        };
        let encoded =
            bincode::serde::encode_to_vec(&document, bincode::config::standard()).unwrap();
        let compressed = zstd::encode_all(&encoded[..], zstd::DEFAULT_COMPRESSION_LEVEL).unwrap();

        let path = temp_bbd_path("rejects-unsupported-version");
        std::fs::write(&path, compressed).unwrap();
        let result = load(&path);
        std::fs::remove_file(&path).ok();

        assert!(matches!(result, Err(LoadError::UnsupportedVersion(v)) if v == FORMAT_VERSION + 1));
    }
}
