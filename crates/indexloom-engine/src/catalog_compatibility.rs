//! Reproducible offline matrix over generated real catalog YAML, including source variants.
use crate::cardigann::{CardigannExecutor, IndexerExecutionConfig};
use indexloom_definitions::{capabilities, parse_yaml};
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    pub path: String,
    pub id: Option<String>,
    pub project: Option<String>,
    pub revision: Option<String>,
    pub stage: String,
    pub error: Option<String>,
    pub capabilities: Option<serde_json::Value>,
    pub configuration_fields: usize,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub total: usize,
    pub failed: usize,
    pub expected_failures: usize,
    pub unexpected_passes: usize,
    pub id_collisions: BTreeMap<String, Vec<String>>,
    pub rows: Vec<Row>,
    pub fixture_cases: Vec<String>,
}
fn files(root: &Path, output: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            files(&entry.path(), output)?;
        } else if kind.is_file()
            && matches!(
                entry.path().extension().and_then(|s| s.to_str()),
                Some("yml" | "yaml")
            )
        {
            output.push(entry.path());
        }
    }
    Ok(())
}
/// Inspect all YAML recursively. Expected failures are exact relative paths and stage names.
///
/// # Errors
/// Returns I/O errors; definition failures are included in the report instead of skipped.
///
/// # Panics
/// Panics if a compiled-in fixture or a serializable definition model is invalid.
pub fn inspect(root: &Path, expected: &BTreeMap<String, String>) -> std::io::Result<Report> {
    let mut paths = Vec::new();
    files(root, &mut paths)?;
    paths.sort();
    let mut rows = Vec::new();
    let mut ids: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut origins = BTreeSet::new();
    for path in paths {
        let relative = path
            .strip_prefix(root)
            .expect("child path")
            .to_string_lossy()
            .replace('\\', "/");
        let mut row = Row {
            path: relative.clone(),
            id: None,
            project: None,
            revision: None,
            stage: "schema".into(),
            error: None,
            capabilities: None,
            configuration_fields: 0,
        };
        match parse_yaml(&fs::read_to_string(path)?) {
            Err(error) => row.error = Some(error.to_string()),
            Ok(definition) => {
                let source = &definition.provenance.source;
                let project = serde_json::to_value(source.project)
                    .expect("project serializes")
                    .as_str()
                    .expect("project string")
                    .to_owned();
                row.id = Some(definition.metadata.id.clone());
                row.project = Some(project.clone());
                row.revision = Some(source.revision.clone());
                ids.entry(definition.metadata.id.clone())
                    .or_default()
                    .push(relative);
                row.stage = "origin".into();
                if origins.insert((project, source.path.clone())) {
                    row.configuration_fields = definition.configuration_schema().fields.len();
                    row.capabilities = Some(
                        serde_json::to_value(capabilities::summarize(&definition))
                            .expect("capabilities serialize"),
                    );
                    row.stage = "runtime-config".into();
                    if let Err(error) = CardigannExecutor::new(IndexerExecutionConfig {
                        indexer_instance_id: "compatibility".into(),
                        definition,
                        values: BTreeMap::new(),
                        secrets: BTreeMap::new(),
                        user_agent: None,
                        proxy: None,
                        flaresolverr: None,
                    }) {
                        row.error = Some(error.to_string());
                    } else {
                        row.stage = "passed".into();
                    }
                } else {
                    row.error = Some("duplicate source project/path".into());
                }
            }
        }
        let raw = root.join(&row.path).with_extension("source.txt");
        if raw.exists()
            && let Err(error) = serde_yaml::from_str::<serde_yaml::Value>(&fs::read_to_string(raw)?)
        {
            row.stage = "raw-yaml".into();
            row.error = Some(error.to_string());
        }
        rows.push(row);
    }
    let failed = rows
        .iter()
        .filter(|row| row.error.is_some() && expected.get(&row.path) != Some(&row.stage))
        .count();
    let expected_failures = rows
        .iter()
        .filter(|row| row.error.is_some() && expected.get(&row.path) == Some(&row.stage))
        .count();
    let unexpected_passes = expected
        .iter()
        .filter(|(path, _)| {
            !rows
                .iter()
                .any(|row| &row.path == *path && row.error.is_some())
        })
        .count();
    Ok(Report {
        total: rows.len(),
        failed,
        expected_failures,
        unexpected_passes,
        id_collisions: ids
            .into_iter()
            .filter(|(_, paths)| paths.len() > 1)
            .collect(),
        rows,
        fixture_cases: fixture_cases(),
    })
}
fn fixture_cases() -> Vec<String> {
    let mut passed = Vec::new();
    for (format, source, body) in [
        (
            "html",
            include_str!("../tests/fixtures/cardigann/html.yml"),
            include_str!("../tests/fixtures/cardigann/html.response"),
        ),
        (
            "json",
            include_str!("../tests/fixtures/cardigann/json.yml"),
            include_str!("../tests/fixtures/cardigann/json.response"),
        ),
        (
            "xml",
            include_str!("../tests/fixtures/cardigann/xml.yml"),
            include_str!("../tests/fixtures/cardigann/xml.response"),
        ),
    ] {
        let mut definition =
            parse_yaml(include_str!("../tests/fixtures/compatibility/example.yml"))
                .expect("fixture schema");
        definition.spec.implementation.search = serde_yaml::from_str(source).expect("fixture YAML");
        if let Ok(rows) = crate::cardigann::compatibility::parse_response(&definition, body, format)
            && rows.len() == 1
            && rows[0].title
                == match format {
                    "html" => "Example Movie",
                    "json" => "Example Movie & More",
                    _ => "Example & Movie",
                }
            && match format {
                "html" => rows[0].size == Some(1_610_612_736),
                "xml" => rows[0].seeders == Some(12),
                _ => rows[0].details_url.as_deref() == Some("https://example.invalid/details/42"),
            }
        {
            passed.push(format.into());
        }
    }
    passed
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_fixture_runs_real_pipeline_and_exact_failure_accounting() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/compatibility");
        let report = inspect(&root, &BTreeMap::new()).unwrap();
        assert_eq!(report.total, 1);
        assert_eq!(report.failed, 0, "{:?}", report.rows);
        assert_eq!(report.fixture_cases, ["html", "json", "xml"]);
        assert!(report.rows[0].capabilities.is_some());
        let expected = BTreeMap::from([("example.yml".into(), "schema".into())]);
        assert_eq!(inspect(&root, &expected).unwrap().unexpected_passes, 1);
    }
}
