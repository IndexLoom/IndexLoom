//! Per-installation local credentials. This module has no web-facing routes;
//! only the packaged startup screen can request the one-time display token.
use std::{fs::{self, OpenOptions}, io::Write, path::Path};
use base64::{engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD}, Engine as _};

pub struct NativeSecrets {
    pub master_key: String,
    pub operator_token: String,
}

/// Never generate a new master key over an existing database. Losing the
/// previous key would make encrypted tracker credentials unrecoverable.
pub fn load_or_create(directory: &Path) -> Result<NativeSecrets, String> {
    fs::create_dir_all(directory).map_err(|_| "cannot create the private app directory")?;
    let master_path = directory.join("master.key");
    if directory.join("indexloom.db").exists() && !master_path.exists() {
        return Err("local database exists but its master key is missing; restore the backup".into());
    }
    let master_key = secret(&master_path, false)?;
    let operator_token = secret(&directory.join("operator.token"), true)?;
    Ok(NativeSecrets { master_key, operator_token })
}

fn secret(path: &Path, url_safe: bool) -> Result<String, String> {
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if !metadata.file_type().is_file() {
            return Err("a native credentials path is not a regular file".into());
        }
        let contents = fs::read_to_string(path)
            .map_err(|_| "could not read private credential file")?;
        if contents.len() > 256 || contents.trim() != contents || contents.len() < 40 {
            return Err("native credential file was corrupted".into());
        }
        return Ok(contents);
    }
    let mut raw = [0_u8; 32];
    getrandom::fill(&mut raw).map_err(|_| "OS random source unavailable")?;
    let value = if url_safe { URL_SAFE_NO_PAD.encode(raw) } else { STANDARD.encode(raw) };
    raw.fill(0);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)
        .map_err(|_| "could not create private credential file")?;
    file.write_all(value.as_bytes())
        .map_err(|_| "could not persist native credentials")?;
    file.sync_all().map_err(|_| "could not sync native credentials")?;
    Ok(value)
}
