//! Scenes: named sets of camera and picture settings the user can switch between in one click.

use serde::{Deserialize, Deserializer, Serialize};

use crate::effects::{Effects, Look};

/// How many scene slots the app shows.
pub const SLOTS: usize = 4;

/// Everything a scene restores. The lens is stored by position and type ("back", "wide")
/// rather than by id, so a scene works with any phone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Scene {
    pub name: String,
    pub position: String,
    pub lens: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub codec: String,
    pub bitrate: u64,
    pub zoom: f64,
    pub mirror: bool,
    pub torch: bool,
    pub stabilization: bool,
    pub effects: Effects,
}

impl Default for Scene {
    fn default() -> Self {
        Scene {
            name: "Scene".into(),
            position: "back".into(),
            lens: "wide".into(),
            width: 1920,
            height: 1080,
            fps: 30,
            codec: "h264".into(),
            bitrate: 8_000_000,
            zoom: 1.0,
            mirror: false,
            torch: false,
            stabilization: true,
            effects: Effects::default(),
        }
    }
}

/// The slots a new install starts with: three ready-made scenes and an empty slot.
pub fn default_scenes() -> Vec<Option<Scene>> {
    vec![
        Some(Scene {
            name: "Meeting".into(),
            effects: Effects { look: Look::Studio, ..Effects::default() },
            ..Scene::default()
        }),
        Some(Scene {
            name: "Streaming".into(),
            fps: 60,
            bitrate: 12_000_000,
            effects: Effects { look: Look::Vivid, ..Effects::default() },
            ..Scene::default()
        }),
        Some(Scene {
            name: "Low light".into(),
            // A lower frame rate lets the camera expose each frame longer.
            fps: 30,
            stabilization: false,
            effects: Effects { brightness: 0.2, contrast: -0.05, ..Effects::default() },
            ..Scene::default()
        }),
        None,
    ]
}

/// Reads the scene slots, keeping exactly `SLOTS` of them; anything unreadable gives the defaults.
pub fn lenient_scenes<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Option<Scene>>, D::Error> {
    let value = serde_json::Value::deserialize(d)?;
    let mut scenes: Vec<Option<Scene>> = serde_json::from_value(value).unwrap_or_else(|_| default_scenes());
    scenes.resize(SLOTS, None);
    for scene in scenes.iter_mut().flatten() {
        scene.effects = scene.effects.clone().sanitized();
    }
    Ok(scenes)
}
