//! Offline real catalog compatibility report command. See docs/stage-d-38-40.md for usage.
use indexloom_engine::catalog_compatibility::inspect;
use std::{collections::BTreeMap, env, fs, path::Path};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args().skip(1).collect();
    if args.len() < 4 {
        return Err(
            "usage: ROOT REPORT_PREFIX MIN_COUNT MAX_FAILURES [EXPECTED_JSON] [variants]".into(),
        );
    }
    let minimum: usize = args[2].parse()?;
    let maximum: usize = args[3].parse()?;
    let expected: BTreeMap<String, String> = if let Some(path) = args.get(4) {
        serde_json::from_str(&fs::read_to_string(path)?)?
    } else {
        BTreeMap::new()
    };
    let report = inspect(Path::new(&args[0]), &expected)?;
    fs::write(
        format!("{}.json", args[1]),
        serde_json::to_string_pretty(&report)?,
    )?;
    let mut markdown = format!(
        "# IndexLoom catalog compatibility\n\nDefinitions: {}\n\nUnexpected failures: {}\n\nExpected failures: {}\n\nUnexpected passes/missing expectations: {}\n\nColliding IDs: {}\n\nOffline response fixtures: {:?}\n\nThis measures schema, configuration and fixture compatibility; it does not contact trackers or prove every definition executes.\n\n| Path | ID | Source revision | Stage |\n| --- | --- | --- | --- |\n",
        report.total,
        report.failed,
        report.expected_failures,
        report.unexpected_passes,
        report.id_collisions.len(),
        report.fixture_cases
    );
    for row in &report.rows {
        use std::fmt::Write;
        writeln!(
            markdown,
            "| {} | {} | {} | {} |",
            row.path.replace('|', "\\|"),
            row.id.as_deref().unwrap_or("?"),
            row.revision.as_deref().unwrap_or("?"),
            row.stage
        )?;
    }
    fs::write(format!("{}.md", args[1]), markdown)?;
    if report.total < minimum
        || report.failed > maximum
        || report.unexpected_passes != 0
        || report.fixture_cases.len() != 3
        || (args.get(5).is_none_or(|mode| mode != "variants") && !report.id_collisions.is_empty())
    {
        return Err(format!(
            "compatibility threshold exceeded: {} definitions, {} failures, {} unexpected passes",
            report.total, report.failed, report.unexpected_passes
        )
        .into());
    }
    println!(
        "{} definitions; {} unexpected failures; {} expected failures",
        report.total, report.failed, report.expected_failures
    );
    Ok(())
}
