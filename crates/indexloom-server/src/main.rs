use std::{
    env,
    fs::File,
    io::Read,
    net::SocketAddr,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use indexloom_server::{
    AdminToken, AppState,
    auth::{AuthMode, AuthSettings, MailSender},
    catalog::{CatalogSource, synchronize},
    router,
};
use indexloom_storage::{SecretVault, Store};
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;
use zeroize::Zeroize;

const MAX_SECRET_FILE_BYTES: usize = 64 * 1024;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("indexloom=info,tower_http=info")),
        )
        .init();

    let bind = env_or("INDEXLOOM_BIND", "0.0.0.0:8080")
        .parse::<SocketAddr>()
        .context("INDEXLOOM_BIND must be a socket address")?;
    let database_url = env_or(
        "INDEXLOOM_DATABASE_URL",
        "sqlite:///data/indexloom.db?mode=rwc",
    );
    let web_root = PathBuf::from(env_or("INDEXLOOM_WEB_ROOT", "/app/web"));
    let mail_sender = MailSender::from_env()?;
    let auth_settings = AuthSettings::from_env(mail_sender.is_some())?;
    let admin_token = load_optional_secret("INDEXLOOM_ADMIN_TOKEN")?
        .map(|mut value| {
            let result = AdminToken::new(&value).map_err(anyhow::Error::msg);
            value.zeroize();
            result
        })
        .transpose()?;
    if matches!(auth_settings.mode, AuthMode::Token | AuthMode::Hybrid) && admin_token.is_none() {
        bail!("token and hybrid modes require INDEXLOOM_ADMIN_TOKEN");
    }
    let mut master_key = load_required_secret("INDEXLOOM_MASTER_KEY")?;
    let vault = SecretVault::from_base64(&master_key);
    master_key.zeroize();
    let vault = vault.context("INDEXLOOM_MASTER_KEY is invalid")?;

    let store = Store::connect(&database_url)
        .await
        .context("failed to initialize IndexLoom storage")?;
    store
        .purge_expired_auth_state()
        .await
        .context("failed to prune expired login state")?;
    let maintenance_store = store.clone();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(3600));
        ticker.tick().await;
        loop {
            ticker.tick().await;
            if let Err(error) = maintenance_store.purge_expired_auth_state().await {
                tracing::warn!(%error, "session housekeeping failed");
            }
        }
    });
    if matches!(auth_settings.mode, AuthMode::Local | AuthMode::Hybrid)
        && store.auth_user_count().await? == 0
        && admin_token.is_none()
    {
        bail!("first local administrator requires INDEXLOOM_ADMIN_TOKEN for bootstrap");
    }
    let catalog_source = CatalogSource::from_values(
        env::var("INDEXLOOM_CATALOG_DIR").ok(),
        &env::var("INDEXLOOM_CATALOG_SOURCE_URL")
            .or_else(|_| env::var("INDEXLOOM_CATALOG_ARCHIVE_URL"))
            .unwrap_or_else(|_| {
                "https://github.com/IndexLoom/Indexers/releases/latest/download/manifest.json"
                    .to_owned()
            }),
    )
    .context("INDEXLOOM catalog source is invalid")?;
    synchronize_catalog_or_retain(&store, &catalog_source).await?;
    let catalog_interval = env_or("INDEXLOOM_CATALOG_SYNC_SECONDS", "21600")
        .parse::<u64>()
        .context("INDEXLOOM_CATALOG_SYNC_SECONDS must be an integer")?;
    let state = AppState::with_auth(
        store.clone(),
        vault,
        admin_token,
        auth_settings,
        mail_sender,
    );
    if catalog_interval != 0 {
        if catalog_interval < 300 {
            anyhow::bail!("INDEXLOOM_CATALOG_SYNC_SECONDS must be 0 or at least 300");
        }
        tokio::spawn(catalog_sync_loop(
            store.clone(),
            catalog_source,
            Duration::from_secs(catalog_interval),
            state.clone(),
        ));
    }
    let listener = TcpListener::bind(bind)
        .await
        .with_context(|| format!("failed to bind {bind}"))?;

    tracing::info!(%bind, "IndexLoom is ready");
    axum::serve(listener, router(state, web_root))
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("IndexLoom HTTP server failed")?;

    Ok(())
}

async fn synchronize_catalog_or_retain(store: &Store, source: &CatalogSource) -> Result<()> {
    match synchronize(store, source).await {
        Ok(report) => {
            tracing::info!(
                definitions = report.definitions,
                new_indexers = report.new_indexers,
                "indexer catalog synchronized"
            );
            Ok(())
        }
        Err(error) => {
            let retained = store
                .definition_count()
                .await
                .context("failed to count retained indexer definitions")?;
            if retained == 0 {
                return Err(error).context("initial indexer catalog synchronization failed");
            }
            tracing::warn!(%error, retained, "catalog sync failed; retaining last valid catalog");
            Ok(())
        }
    }
}

async fn catalog_sync_loop(
    store: Store,
    source: CatalogSource,
    interval: Duration,
    state: AppState,
) {
    let mut ticker = tokio::time::interval(interval);
    ticker.tick().await;
    loop {
        ticker.tick().await;
        match synchronize(&store, &source).await {
            Ok(report) if report.updated => state.clear_execution_runtimes().await,
            Ok(_) => {}
            Err(error) => tracing::warn!(%error, "scheduled catalog synchronization failed"),
        }
    }
}

fn env_or(key: &str, default: &str) -> String {
    env::var(key).unwrap_or_else(|_| default.to_owned())
}

fn load_optional_secret(name: &str) -> Result<Option<String>> {
    let file_name = format!("{name}_FILE");
    if env::var_os(name).is_none() && env::var_os(&file_name).is_none() {
        return Ok(None);
    }
    load_required_secret(name).map(Some)
}

fn load_required_secret(name: &str) -> Result<String> {
    let file_name = format!("{name}_FILE");
    let direct = env::var_os(name)
        .map(|value| {
            value
                .into_string()
                .map_err(|_| anyhow::anyhow!("{name} must contain valid UTF-8"))
        })
        .transpose()?;
    let file = env::var_os(&file_name).map(PathBuf::from);
    load_secret_from_sources(name, direct, file.as_deref())
}

fn load_secret_from_sources(
    name: &str,
    direct: Option<String>,
    file: Option<&Path>,
) -> Result<String> {
    let file_name = format!("{name}_FILE");
    match (direct, file) {
        (Some(mut value), Some(_)) => {
            value.zeroize();
            bail!("set exactly one of {name} or {file_name}, not both");
        }
        (None, None) => bail!("one of {name} or {file_name} is required"),
        (Some(value), None) => {
            if value.is_empty() {
                bail!("{name} must not be empty");
            }
            Ok(value)
        }
        (None, Some(path)) => read_secret_file(name, &file_name, path),
    }
}

fn read_secret_file(name: &str, file_name: &str, path: &Path) -> Result<String> {
    if path.as_os_str().is_empty() {
        bail!("{file_name} must not be empty");
    }
    let file = File::open(path).with_context(|| format!("could not open {file_name}"))?;
    let metadata = file
        .metadata()
        .with_context(|| format!("could not inspect {file_name}"))?;
    if !metadata.is_file() {
        bail!("{file_name} must point to a regular file");
    }
    let max_file_bytes = u64::try_from(MAX_SECRET_FILE_BYTES).expect("64 KiB fits in u64");
    if metadata.len() > max_file_bytes {
        bail!("{file_name} exceeds the 64 KiB limit");
    }

    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    if let Err(error) = file.take(max_file_bytes + 1).read_to_end(&mut bytes) {
        bytes.zeroize();
        return Err(error).with_context(|| format!("could not read {file_name}"));
    }
    if bytes.len() > MAX_SECRET_FILE_BYTES {
        bytes.zeroize();
        bail!("{file_name} exceeds the 64 KiB limit");
    }
    let mut value = match String::from_utf8(bytes) {
        Ok(value) => value,
        Err(error) => {
            let mut invalid = error.into_bytes();
            invalid.zeroize();
            bail!("{file_name} must contain valid UTF-8");
        }
    };
    if value.ends_with("\r\n") {
        value.truncate(value.len() - 2);
    } else if value.ends_with('\n') {
        value.truncate(value.len() - 1);
    }
    if value.is_empty() {
        value.zeroize();
        bail!("{name} must not be empty");
    }
    Ok(value)
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::error!(%error, "failed to install Ctrl+C handler");
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(error) => tracing::error!(%error, "failed to install SIGTERM handler"),
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }

    tracing::info!("shutdown signal received");
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use uuid::Uuid;
    use zeroize::Zeroize;

    use super::{MAX_SECRET_FILE_BYTES, load_secret_from_sources};

    #[test]
    fn loads_a_direct_secret_verbatim() {
        let secret = load_secret_from_sources(
            "INDEXLOOM_TEST_SECRET",
            Some(" value with spaces ".to_owned()),
            None,
        )
        .expect("direct secret should load");
        assert_eq!(secret, " value with spaces ");
    }

    #[test]
    fn rejects_ambiguous_or_missing_sources() {
        let file = temporary_path("ambiguous");
        fs::write(&file, "file secret").unwrap();
        let ambiguous = load_secret_from_sources(
            "INDEXLOOM_TEST_SECRET",
            Some("direct secret".to_owned()),
            Some(&file),
        )
        .expect_err("two sources must fail");
        let missing = load_secret_from_sources("INDEXLOOM_TEST_SECRET", None, None)
            .expect_err("no source must fail");
        assert!(ambiguous.to_string().contains("not both"));
        assert!(missing.to_string().contains("is required"));
        fs::remove_file(file).unwrap();
    }

    #[test]
    fn rejects_empty_sources_and_hides_file_paths() {
        let empty_direct =
            load_secret_from_sources("INDEXLOOM_TEST_SECRET", Some(String::new()), None)
                .expect_err("empty direct secret must fail");
        let empty_path = PathBuf::new();
        let empty_file = load_secret_from_sources("INDEXLOOM_TEST_SECRET", None, Some(&empty_path))
            .expect_err("empty secret path must fail");
        let missing = temporary_path("missing-sensitive-path");
        let missing_error = load_secret_from_sources("INDEXLOOM_TEST_SECRET", None, Some(&missing))
            .expect_err("missing secret file must fail");
        assert!(empty_direct.to_string().contains("must not be empty"));
        assert!(empty_file.to_string().contains("must not be empty"));
        assert!(!format!("{missing_error:#}").contains("missing-sensitive-path"));
    }

    #[test]
    fn strips_exactly_one_secret_file_line_ending() {
        let file = temporary_path("line-ending");
        fs::write(&file, "secret-value\r\n").unwrap();
        let mut secret = load_secret_from_sources("INDEXLOOM_TEST_SECRET", None, Some(&file))
            .expect("secret file should load");
        assert_eq!(secret, "secret-value");
        secret.zeroize();

        fs::write(&file, "secret-value\n\n").unwrap();
        let mut secret = load_secret_from_sources("INDEXLOOM_TEST_SECRET", None, Some(&file))
            .expect("secret file should load");
        assert_eq!(secret, "secret-value\n");
        secret.zeroize();
        fs::remove_file(file).unwrap();
    }

    #[test]
    fn rejects_non_regular_and_oversized_secret_files() {
        let directory = temporary_path("directory");
        fs::create_dir(&directory).unwrap();
        let non_regular = load_secret_from_sources("INDEXLOOM_TEST_SECRET", None, Some(&directory))
            .expect_err("directory must fail");
        assert!(non_regular.to_string().contains("regular file"));
        fs::remove_dir(directory).unwrap();

        let file = temporary_path("oversized");
        fs::write(&file, vec![b'x'; MAX_SECRET_FILE_BYTES + 1]).unwrap();
        let oversized = load_secret_from_sources("INDEXLOOM_TEST_SECRET", None, Some(&file))
            .expect_err("oversized secret must fail");
        assert!(oversized.to_string().contains("64 KiB"));
        fs::remove_file(file).unwrap();
    }

    #[test]
    fn accepts_the_exact_limit_and_rejects_invalid_utf8() {
        let file = temporary_path("exact-limit");
        fs::write(&file, vec![b'x'; MAX_SECRET_FILE_BYTES]).unwrap();
        let mut secret = load_secret_from_sources("INDEXLOOM_TEST_SECRET", None, Some(&file))
            .expect("exactly 64 KiB should be accepted");
        assert_eq!(secret.len(), MAX_SECRET_FILE_BYTES);
        secret.zeroize();

        fs::write(&file, [0xff, 0xfe]).unwrap();
        let invalid = load_secret_from_sources("INDEXLOOM_TEST_SECRET", None, Some(&file))
            .expect_err("invalid UTF-8 must fail");
        assert!(invalid.to_string().contains("valid UTF-8"));
        fs::remove_file(file).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn follows_a_platform_managed_secret_symlink() {
        use std::os::unix::fs::symlink;

        let target = temporary_path("symlink-target");
        let intermediate = temporary_path("symlink-intermediate");
        let link = temporary_path("symlink");
        fs::write(&target, "mounted-secret\n").unwrap();
        symlink(&target, &intermediate).unwrap();
        symlink(&intermediate, &link).unwrap();
        let mut secret = load_secret_from_sources("INDEXLOOM_TEST_SECRET", None, Some(&link))
            .expect("symlinked secret should load");
        assert_eq!(secret, "mounted-secret");
        secret.zeroize();
        fs::remove_file(link).unwrap();
        fs::remove_file(intermediate).unwrap();
        fs::remove_file(target).unwrap();
    }

    fn temporary_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("indexloom-{label}-{}", Uuid::now_v7()))
    }
}
