//! Project container directory structure.
//!
//! Defines the standardised directory layout for project containers
//! and maps tool slugs to their output directories.

/// IFC/BIM models.
pub const DIR_MODELS: &str = "models";
/// BCF issue sets.
pub const DIR_ISSUES: &str = "issues";
/// Generated reports (PDF).
pub const DIR_REPORTS: &str = "reports";
/// Calculation results (heat loss, etc.).
pub const DIR_CALCULATIONS: &str = "calculations";
/// BIM validation results.
pub const DIR_VALIDATION: &str = "validation";
/// 2D drawings.
pub const DIR_DRAWINGS: &str = "drawings";
/// Default manifest filename (backward compatibility).
pub const DEFAULT_MANIFEST_FILENAME: &str = "project.wefc";

/// Manifest file extension.
pub const MANIFEST_EXTENSION: &str = ".wefc";

/// Legacy subdirectory used before the project container model.
pub const LEGACY_SUBDIR: &str = "99_overige_documenten";

/// Map a tool slug to its output directory in the project container.
///
/// # Examples
///
/// ```
/// use openaec_cloud::container::output_dir_for_tool;
/// assert_eq!(output_dir_for_tool("bcf-platform"), "issues");
/// assert_eq!(output_dir_for_tool("bim-validator"), "validation");
/// ```
pub fn output_dir_for_tool(tool: &str) -> &'static str {
    match tool {
        "bcf-platform" => DIR_ISSUES,
        "bim-validator" => DIR_VALIDATION,
        "warmteverlies" | "isso51" => DIR_CALCULATIONS,
        "reports" | "openaec-reports" => DIR_REPORTS,
        "2d-studio" | "open-2d-studio" => DIR_DRAWINGS,
        "pdf-studio" | "open-pdf-studio" => DIR_REPORTS,
        _ => DIR_REPORTS, // fallback
    }
}
