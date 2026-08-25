use std::{collections::BTreeSet, env, fs, path::PathBuf, process::ExitCode};

use indexloom_definitions::parse_yaml;

fn main() -> ExitCode {
    match validate_catalog() {
        Ok(report) => {
            println!(
                "Validated {} IndexLoom definition(s), {} secret field(s), and {} FlareSolverr-aware definition(s) with the Rust model.",
                report.definitions, report.secret_fields, report.flaresolverr_definitions
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("Catalog validation failed: {error}");
            ExitCode::FAILURE
        }
    }
}

struct Report {
    definitions: usize,
    secret_fields: usize,
    flaresolverr_definitions: usize,
}

fn validate_catalog() -> Result<Report, String> {
    let mut arguments = env::args_os().skip(1);
    let directory = PathBuf::from(
        arguments
            .next()
            .ok_or_else(|| "usage: validate_catalog <definitions-directory>".to_owned())?,
    );
    if arguments.next().is_some() {
        return Err("usage: validate_catalog <definitions-directory>".to_owned());
    }

    let mut paths = fs::read_dir(&directory)
        .map_err(|error| format!("cannot read {}: {error}", directory.display()))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "yml"))
        .collect::<Vec<_>>();
    paths.sort();

    let mut identifiers = BTreeSet::new();
    let mut secret_fields = 0;
    let mut flaresolverr_definitions = 0;
    for path in &paths {
        let metadata = fs::symlink_metadata(path)
            .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(format!("refusing non-regular file {}", path.display()));
        }
        let source = fs::read_to_string(path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let definition =
            parse_yaml(&source).map_err(|error| format!("{}: {error}", path.display()))?;
        let stem = path
            .file_stem()
            .and_then(|value| value.to_str())
            .ok_or_else(|| format!("invalid filename {}", path.display()))?;
        if stem != definition.metadata.id {
            return Err(format!(
                "{} does not match metadata.id {}",
                path.display(),
                definition.metadata.id
            ));
        }
        if !identifiers.insert(definition.metadata.id.clone()) {
            return Err(format!("duplicate metadata.id {}", definition.metadata.id));
        }
        let configuration = definition.configuration_schema();
        let mut field_names = BTreeSet::new();
        for field in configuration.fields {
            if !field_names.insert(field.name.clone()) {
                return Err(format!(
                    "{} contains duplicate configuration field {}",
                    path.display(),
                    field.name
                ));
            }
            secret_fields += usize::from(field.secret);
        }
        flaresolverr_definitions += usize::from(configuration.flaresolverr_recommended);
    }
    Ok(Report {
        definitions: paths.len(),
        secret_fields,
        flaresolverr_definitions,
    })
}
