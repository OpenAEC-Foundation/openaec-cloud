//! Project manifest (`.wefc`) read/write/merge operations.
//!
//! The manifest tracks all tool outputs within a project container
//! using WeFC-compatible JSON objects identified by `guid`.
//!
//! A project can contain **multiple** `.wefc` manifest files — each is a
//! user-curated "playlist" of data objects. The default manifest filename
//! is `project.wefc` (see [`DEFAULT_MANIFEST_FILENAME`]).

use serde::{Deserialize, Serialize};

use crate::container::DEFAULT_MANIFEST_FILENAME;

/// A project manifest (`.wefc` file).
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ProjectManifest {
    pub header: ManifestHeader,
    #[serde(default)]
    pub data: Vec<serde_json::Value>,
}

/// Manifest header with schema info and last-write metadata.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ManifestHeader {
    /// Always `"WeFC"`.
    pub schema: String,
    /// Schema version, e.g. `"1.1.0"`.
    #[serde(rename = "schemaVersion")]
    pub schema_version: String,
    /// Unique file identifier (UUID v4).
    #[serde(rename = "fileId")]
    pub file_id: String,
    /// Optional human-readable description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// ISO 8601 timestamp of last modification.
    pub timestamp: String,
    /// Tool name that last wrote the manifest.
    pub application: String,
    /// Version of the application that last wrote the manifest.
    #[serde(rename = "applicationVersion", skip_serializing_if = "Option::is_none")]
    pub application_version: Option<String>,
}

impl ProjectManifest {
    /// Create a new empty manifest for a given application.
    ///
    /// Generates a UUID v4 `fileId` for the manifest header.
    pub fn new(application: &str) -> Self {
        Self {
            header: ManifestHeader {
                schema: "WeFC".to_string(),
                schema_version: "1.1.0".to_string(),
                file_id: uuid::Uuid::new_v4().to_string(),
                description: None,
                timestamp: now_iso8601(),
                application: application.to_string(),
                application_version: None,
            },
            data: Vec::new(),
        }
    }

    /// Find all objects matching a given `type` field.
    pub fn find_by_type(&self, type_name: &str) -> Vec<&serde_json::Value> {
        self.data
            .iter()
            .filter(|obj| {
                obj.get("type")
                    .and_then(|v| v.as_str())
                    .is_some_and(|t| t == type_name)
            })
            .collect()
    }

    /// Find an object by its `guid` field.
    pub fn find_by_guid(&self, guid: &str) -> Option<&serde_json::Value> {
        self.data.iter().find(|obj| {
            obj.get("guid")
                .and_then(|v| v.as_str())
                .is_some_and(|g| g == guid)
        })
    }

    /// Find an object by its `path` field.
    ///
    /// Useful for merge-by-path instead of merge-by-guid when correlating
    /// objects across multiple manifest files.
    pub fn find_by_path(&self, path: &str) -> Option<&serde_json::Value> {
        self.data.iter().find(|obj| {
            obj.get("path")
                .and_then(|v| v.as_str())
                .is_some_and(|p| p == path)
        })
    }

    /// Add a new object or update an existing one (matched by `guid`).
    ///
    /// Returns `true` if an existing object was updated, `false` if added.
    pub fn add_or_update(&mut self, object: serde_json::Value) -> bool {
        let guid = object
            .get("guid")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        if let Some(ref guid) = guid {
            if let Some(pos) = self.data.iter().position(|obj| {
                obj.get("guid")
                    .and_then(|v| v.as_str())
                    .is_some_and(|g| g == guid)
            }) {
                self.data[pos] = object;
                self.touch();
                return true;
            }
        }

        self.data.push(object);
        self.touch();
        false
    }

    /// Remove an object by `guid`. Returns `true` if found and removed.
    pub fn remove_by_guid(&mut self, guid: &str) -> bool {
        let before = self.data.len();
        self.data.retain(|obj| {
            obj.get("guid")
                .and_then(|v| v.as_str())
                .map_or(true, |g| g != guid)
        });
        let removed = self.data.len() < before;
        if removed {
            self.touch();
        }
        removed
    }

    /// Number of objects in the manifest.
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Check if the manifest has no data objects.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Serialize to pretty-printed JSON bytes.
    pub fn to_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec_pretty(self)
    }

    /// Deserialize from JSON bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }

    /// The default filename for project manifests (`project.wefc`).
    pub fn default_filename() -> &'static str {
        DEFAULT_MANIFEST_FILENAME
    }

    /// Update the header timestamp and application.
    fn touch(&mut self) {
        self.header.timestamp = now_iso8601();
    }
}

/// Simple ISO 8601 UTC timestamp (no chrono dependency).
fn now_iso8601() -> String {
    let dur = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = dur.as_secs();

    let days = secs / 86400;
    let time_secs = secs % 86400;
    let hours = time_secs / 3600;
    let minutes = (time_secs % 3600) / 60;
    let seconds = time_secs % 60;

    let mut y = 1970i64;
    let mut remaining = days as i64;
    loop {
        let diy = if is_leap(y) { 366 } else { 365 };
        if remaining < diy {
            break;
        }
        remaining -= diy;
        y += 1;
    }

    let md: [i64; 12] = if is_leap(y) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    let mut m = 0usize;
    for (i, &d) in md.iter().enumerate() {
        if remaining < d {
            m = i;
            break;
        }
        remaining -= d;
    }

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        y,
        m + 1,
        remaining + 1,
        hours,
        minutes,
        seconds
    )
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}
