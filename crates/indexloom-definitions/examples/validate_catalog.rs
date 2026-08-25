use std::{collections::BTreeSet, env, fs, path::PathBuf, process::ExitCode};

use indexloom_definitions::parse_yaml;

fn main() -> ExitCode {
    match validate_catalog() {
        Ok(count) => {
            println!("Validated {count} IndexLoom definition(s) with the Rust model.");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("Catalog validation failed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn validate_catalog() -> Result<usize, String> {
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
    }
    Ok(paths.len())
}
