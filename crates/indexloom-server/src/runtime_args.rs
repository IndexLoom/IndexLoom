//! Cross-platform flags for standalone Rust, Docker, Windows, Linux and macOS.
//! Native Tauri frontends use their mode chooser instead of reading argv.
use indexloom_core::deployment::{Deployment, RuntimeMode};

pub const HELP: &str = "\
IndexLoom — local and remote operation

USAGE:
  indexloom-server [--mode autonomous|external] [--remote HTTPS_URL]

OPTIONS:
  --mode MODE     autonomous (default): full local SQLite + indexer engine
                  external: no local database; redirect to remote web IndexLoom
  --remote URL    HTTPS root URL of an existing IndexLoom deployment
                  Implies --mode external when --mode is omitted
  -h, --help      Show this help

ENVIRONMENT:
  INDEXLOOM_RUNTIME_MODE   autonomous or external
  INDEXLOOM_REMOTE_URL     HTTPS remote URL for external mode
  INDEXLOOM_BIND           local listener (default 0.0.0.0:8080)
";

pub enum Command {
    Help,
    Start(Deployment),
}

/// Combines explicit CLI options with environment fallbacks.
/// The CLI never accepts raw passwords or API tokens.
///
/// # Errors
/// Rejects missing or unexpected arguments and unsafe remote addresses.
pub fn parse_from_environment<I>(arguments: I) -> Result<Command, String>
where
    I: IntoIterator<Item = String>,
{
    parse(
        arguments,
        std::env::var("INDEXLOOM_RUNTIME_MODE").ok().as_deref(),
        std::env::var("INDEXLOOM_REMOTE_URL").ok().as_deref(),
    )
}

fn parse<I>(arguments: I, env_mode: Option<&str>, env_remote: Option<&str>) -> Result<Command, String>
where
    I: IntoIterator<Item = String>,
{
    let mut args = arguments.into_iter();
    let mut mode = None;
    let mut remote = None;

    while let Some(argument) = args.next() {
        match argument.as_str() {
            "-h" | "--help" => {
                if mode.is_some() || remote.is_some() || args.next().is_some() {
                    return Err("--help cannot be combined with other flags".into());
                }
                return Ok(Command::Help);
            }
            "--mode" => {
                if mode.is_some() {
                    return Err("--mode was provided more than once".into());
                }
                let raw = args.next().ok_or("--mode requires a value")?;
                mode = Some(RuntimeMode::parse(&raw)?);
            }
            "--remote" => {
                if remote.is_some() {
                    return Err("--remote was provided more than once".into());
                }
                remote = Some(args.next().ok_or("--remote requires a value")?);
            }
            _ => return Err(format!("unknown argument: {argument}; use --help")),
        }
    }

    let selected = if let Some(mode) = mode {
        mode
    } else if remote.is_some() {
        RuntimeMode::External
    } else {
        RuntimeMode::parse(env_mode.unwrap_or("autonomous"))?
    };

    match selected {
        RuntimeMode::Autonomous if remote.is_none() => {
            Ok(Command::Start(Deployment::autonomous()))
        }
        RuntimeMode::Autonomous => Err("--remote is not valid with autonomous mode".into()),
        RuntimeMode::External => {
            let url = remote.as_deref().or(env_remote).ok_or(
                "external mode requires --remote URL or INDEXLOOM_REMOTE_URL",
            )?;
            Deployment::external(url).map(Command::Start)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Command, parse};
    use indexloom_core::deployment::RuntimeMode;

    fn args(raw: &[&str], env_mode: Option<&str>, env_url: Option<&str>) -> Result<Command, String> {
        parse(raw.iter().map(|v| (*v).to_owned()), env_mode, env_url)
    }

    #[test]
    fn defaults_to_autonomous_without_environment_or_cli_flags() {
        let Command::Start(deployment) = args(&[], None, None).unwrap() else {
            panic!("expected runtime");
        };
        assert_eq!(deployment.mode, RuntimeMode::Autonomous);
    }

    #[test]
    fn cli_flags_override_environment_without_leaking_urls() {
        let Command::Start(deployment) = args(
            &["--remote", "https://example.org/"],
            Some("autonomous"),
            None
        ).unwrap() else {
            panic!("expected runtime");
        };
        assert_eq!(deployment.mode, RuntimeMode::External);
        assert_eq!(deployment.remote.unwrap().as_str(), "https://example.org/");
        let Command::Start(deployment) = args(
            &["--mode", "autonomous"], Some("external"), Some("https://other.org/")
        ).unwrap() else {
            panic!("expected runtime");
        };
        assert_eq!(deployment.mode, RuntimeMode::Autonomous);
    }

    #[test]
    fn requires_strict_modes_and_https_root_urls() {
        assert!(args(&["--mode", "external"], None, None).is_err());
        assert!(args(&["--mode", "autonomous", "--remote", "https://example.org"], None, None).is_err());
        assert!(args(&["--remote", "http://example.org"], None, None).is_err());
        assert!(args(&["--remote", "https://user:pass@example.org"], None, None).is_err());
        assert!(args(&["--mode"], None, None).is_err());
        assert!(args(&["--mode", "local", "--mode", "external"], None, None).is_err());
    }

    #[test]
    fn help_and_bad_arguments_do_not_start_a_server() {
        assert!(matches!(args(&["--help"], None, None).unwrap(), Command::Help));
        assert!(args(&["--help", "--remote", "https://example.org"], None, None).is_err());
        assert!(args(&["--unknown"], None, None).is_err());
    }
}
