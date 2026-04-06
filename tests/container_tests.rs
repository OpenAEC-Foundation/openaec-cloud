use openaec_cloud::container::output_dir_for_tool;
use openaec_cloud::ProjectManifest;

#[test]
fn tool_mapping_bcf() {
    assert_eq!(output_dir_for_tool("bcf-platform"), "issues");
}

#[test]
fn tool_mapping_bim_validator() {
    assert_eq!(output_dir_for_tool("bim-validator"), "validation");
}

#[test]
fn tool_mapping_warmteverlies() {
    assert_eq!(output_dir_for_tool("warmteverlies"), "calculations");
    assert_eq!(output_dir_for_tool("isso51"), "calculations");
}

#[test]
fn tool_mapping_reports() {
    assert_eq!(output_dir_for_tool("reports"), "reports");
    assert_eq!(output_dir_for_tool("openaec-reports"), "reports");
}

#[test]
fn tool_mapping_drawings() {
    assert_eq!(output_dir_for_tool("2d-studio"), "drawings");
    assert_eq!(output_dir_for_tool("open-2d-studio"), "drawings");
}

#[test]
fn tool_mapping_pdf_studio() {
    assert_eq!(output_dir_for_tool("pdf-studio"), "reports");
    assert_eq!(output_dir_for_tool("open-pdf-studio"), "reports");
}

#[test]
fn tool_mapping_unknown_falls_back_to_reports() {
    assert_eq!(output_dir_for_tool("unknown-tool"), "reports");
}

#[test]
fn manifest_new_creates_empty() {
    let m = ProjectManifest::new("bim-validator");
    assert_eq!(m.header.schema, "WeFC");
    assert_eq!(m.header.schema_version, "1.0.0");
    assert_eq!(m.header.application, "bim-validator");
    assert!(m.is_empty());
    assert_eq!(m.len(), 0);
}

#[test]
fn manifest_add_and_find_by_guid() {
    let mut m = ProjectManifest::new("test");
    let obj = serde_json::json!({
        "type": "WefcValidation",
        "guid": "abc-123",
        "name": "Test validation"
    });

    let updated = m.add_or_update(obj);
    assert!(!updated); // was added, not updated
    assert_eq!(m.len(), 1);

    let found = m.find_by_guid("abc-123");
    assert!(found.is_some());
    assert_eq!(
        found.unwrap().get("name").unwrap().as_str().unwrap(),
        "Test validation"
    );

    assert!(m.find_by_guid("nonexistent").is_none());
}

#[test]
fn manifest_find_by_type() {
    let mut m = ProjectManifest::new("test");
    m.add_or_update(serde_json::json!({
        "type": "WefcValidation", "guid": "v1"
    }));
    m.add_or_update(serde_json::json!({
        "type": "WefcReport", "guid": "r1"
    }));
    m.add_or_update(serde_json::json!({
        "type": "WefcValidation", "guid": "v2"
    }));

    let validations = m.find_by_type("WefcValidation");
    assert_eq!(validations.len(), 2);

    let reports = m.find_by_type("WefcReport");
    assert_eq!(reports.len(), 1);

    let empty = m.find_by_type("WefcDrawing");
    assert!(empty.is_empty());
}

#[test]
fn manifest_update_existing() {
    let mut m = ProjectManifest::new("test");
    m.add_or_update(serde_json::json!({
        "type": "WefcReport",
        "guid": "r1",
        "name": "Original"
    }));

    let updated = m.add_or_update(serde_json::json!({
        "type": "WefcReport",
        "guid": "r1",
        "name": "Updated"
    }));
    assert!(updated); // was updated
    assert_eq!(m.len(), 1); // still only one object

    let found = m.find_by_guid("r1").unwrap();
    assert_eq!(found.get("name").unwrap().as_str().unwrap(), "Updated");
}

#[test]
fn manifest_remove_by_guid() {
    let mut m = ProjectManifest::new("test");
    m.add_or_update(serde_json::json!({"guid": "a"}));
    m.add_or_update(serde_json::json!({"guid": "b"}));
    assert_eq!(m.len(), 2);

    assert!(m.remove_by_guid("a"));
    assert_eq!(m.len(), 1);
    assert!(m.find_by_guid("a").is_none());
    assert!(m.find_by_guid("b").is_some());

    assert!(!m.remove_by_guid("nonexistent"));
    assert_eq!(m.len(), 1);
}

#[test]
fn manifest_serialization_roundtrip() {
    let mut m = ProjectManifest::new("bcf-platform");
    m.add_or_update(serde_json::json!({
        "type": "WefcIssueSet",
        "guid": "issue-1",
        "name": "Architectural Review",
        "path": "issues/review.bcfzip",
        "status": "active"
    }));

    let bytes = m.to_bytes().unwrap();
    let restored = ProjectManifest::from_bytes(&bytes).unwrap();

    assert_eq!(restored.header.schema, "WeFC");
    assert_eq!(restored.header.application, "bcf-platform");
    assert_eq!(restored.len(), 1);
    assert_eq!(
        restored.find_by_guid("issue-1").unwrap()
            .get("name").unwrap().as_str().unwrap(),
        "Architectural Review"
    );
}

#[test]
fn manifest_filename_is_project_wefc() {
    assert_eq!(ProjectManifest::filename(), "project.wefc");
}
