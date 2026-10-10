//! Finds and reads what the person picked (the C# `MudletSourceReader`): a Mudlet profile folder
//! (its latest saved profile in `current/` and its connection files), a profile or package
//! `.xml`, or a zipped `.mpackage`. Nothing is written and nothing is run. A stored password file
//! is noticed, never opened.

use std::fs;
use std::path::Path;
use std::time::SystemTime;

use super::model::{ImportError, Package, Source};
use super::parser;
use super::zip::{ZipArchive, ZipError};
use crate::l10n::{S, t, tf};

pub const MAX_XML_BYTES: u64 = 32 * 1024 * 1024;
pub const MAX_ARCHIVE_BYTES: u64 = 256 * 1024 * 1024;
pub const MAX_EXPANDED_BYTES: u64 = 128 * 1024 * 1024;
pub const MAX_ARCHIVE_ENTRIES: usize = 4096;
pub const MAX_PACKAGE_DOCUMENTS: usize = 16;
const MAX_SETTING_BYTES: u64 = 4096;

/// Read a profile folder, a profile or package file, or a zipped package.
pub fn read(path: &Path) -> Result<Source, ImportError> {
    if path.is_dir() {
        return read_folder(path);
    }
    if !path.is_file() {
        return Err(ImportError::new(S::MudletImportNothingFound));
    }
    let extension = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "mpackage" | "zip" => {
            let data = read_file(path, MAX_ARCHIVE_BYTES)?;
            read_archive(&data, &stem(path))
        }
        "xml" => read_xml_file(path),
        _ => Err(ImportError::new(S::MudletImportUnknownFile)),
    }
}

fn stem(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn read_failed(error: std::io::Error) -> ImportError {
    ImportError(tf(S::MudletImportReadFailed, &[&error]))
}

fn read_file(path: &Path, limit: u64) -> Result<Vec<u8>, ImportError> {
    let size = fs::metadata(path).map_err(read_failed)?.len();
    if size > limit {
        return Err(ImportError::new(S::MudletImportTooLarge));
    }
    let data = fs::read(path).map_err(read_failed)?;
    if data.len() as u64 > limit {
        return Err(ImportError::new(S::MudletImportTooLarge));
    }
    Ok(data)
}

fn parse_file(path: &Path) -> Result<Package, ImportError> {
    parser::parse(&read_file(path, MAX_XML_BYTES)?)
}

fn read_xml_file(path: &Path) -> Result<Source, ImportError> {
    let package = parse_file(path)?;
    // A file inside a profile's current/ folder is that profile's save: its connection files
    // sit two levels up.
    let full = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if package.has_host
        && let Some(folder) = full.parent()
        && folder.file_name().is_some_and(|n| n == "current")
        && let Some(profile) = folder.parent()
    {
        return Ok(from_profile(profile, package));
    }
    let name = if package.has_host {
        package.host_name.clone()
    } else {
        stem(path)
    };
    Ok(from_packages(&name, vec![package]))
}

fn read_folder(folder: &Path) -> Result<Source, ImportError> {
    let current = folder.join("current");
    let mut saves: Vec<(SystemTime, String, std::path::PathBuf)> = Vec::new();
    if let Ok(entries) = fs::read_dir(&current) {
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            if !meta.is_file() || path.extension().is_none_or(|e| !e.eq_ignore_ascii_case("xml")) {
                continue;
            }
            let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            saves.push((modified, entry.file_name().to_string_lossy().into_owned(), path));
        }
    }
    // Mudlet names each save by its date and time; the most recently written one is the
    // profile as it is now.
    saves.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    let Some((_, _, latest)) = saves.first() else {
        return Err(ImportError::new(S::MudletImportNoProfileSave));
    };
    Ok(from_profile(folder, parse_file(latest)?))
}

fn from_profile(folder: &Path, package: Package) -> Source {
    let url = setting(folder, "url");
    let port = setting(folder, "port");
    let port = if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) {
        port.parse::<u16>().ok()
    } else {
        None
    };
    // Mudlet writes the TLS choice as a check state, 2 meaning checked.
    let tls_setting = setting(folder, "ssl_tsl");
    let tls = if tls_setting.is_empty() {
        package.tls
    } else {
        Some(tls_setting == "2")
    };
    let name = if package.host_name.trim().is_empty() {
        folder
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    } else {
        package.host_name.trim().to_string()
    };
    Source {
        name,
        is_profile: true,
        host: if url.is_empty() { package.url.clone() } else { url },
        port: port.or(package.port),
        tls,
        login: setting(folder, "login"),
        encoding: setting(folder, "encoding"),
        password_skipped: folder.join("password").exists(),
        packages: vec![package],
    }
}

fn from_packages(name: &str, packages: Vec<Package>) -> Source {
    let host = packages.iter().find(|p| p.has_host);
    Source {
        name: if name.trim().is_empty() {
            t(S::MudletImportUnnamed).to_string()
        } else {
            name.trim().to_string()
        },
        is_profile: host.is_some(),
        host: host.map(|h| h.url.clone()).unwrap_or_default(),
        port: host.and_then(|h| h.port),
        tls: host.and_then(|h| h.tls),
        packages,
        ..Source::default()
    }
}

/// One of the small text files Mudlet keeps beside a profile. Anything unexpected reads as
/// empty; a link is not followed.
fn setting(folder: &Path, name: &str) -> String {
    let path = folder.join(name);
    let Ok(meta) = fs::symlink_metadata(&path) else {
        return String::new();
    };
    if !meta.is_file() || meta.len() > MAX_SETTING_BYTES {
        return String::new();
    }
    let Ok(text) = fs::read_to_string(&path) else {
        return String::new();
    };
    let text = text.trim();
    if text.chars().any(char::is_control) {
        String::new()
    } else {
        text.to_string()
    }
}

fn archive_error(error: ZipError) -> ImportError {
    ImportError::new(match error {
        ZipError::Invalid => S::MudletImportBadArchive,
        ZipError::TooLarge => S::MudletImportTooLarge,
    })
}

/// Read a zipped package from memory. Every entry name is checked before anything is read: an
/// absolute path, a parent reference or a device name refuses the whole archive, as does an
/// archive whose entries would expand past the limits. Only the package's XML and its
/// `config.lua` are read.
pub fn read_archive(data: &[u8], fallback_name: &str) -> Result<Source, ImportError> {
    if data.len() as u64 > MAX_ARCHIVE_BYTES {
        return Err(ImportError::new(S::MudletImportTooLarge));
    }
    let archive = ZipArchive::open(data, MAX_ARCHIVE_ENTRIES).map_err(archive_error)?;
    let mut declared: u64 = 0;
    for entry in &archive.entries {
        if !is_safe_entry_name(&entry.name) {
            return Err(ImportError::new(S::MudletImportUnsafeArchive));
        }
        declared = declared.saturating_add(entry.size);
        if declared > MAX_EXPANDED_BYTES {
            return Err(ImportError::new(S::MudletImportTooLarge));
        }
    }
    let mut name = fallback_name.to_string();
    if let Some(config) = archive
        .entries
        .iter()
        .find(|e| e.name.eq_ignore_ascii_case("config.lua"))
    {
        let text = archive.read(config, MAX_SETTING_BYTES * 16).map_err(archive_error)?;
        if let Some(found) = package_name(&String::from_utf8_lossy(&text)) {
            name = found;
        }
    }
    // The package's own XML sits at the top of the archive; resources in folders are not packages.
    let mut documents: Vec<_> = archive
        .entries
        .iter()
        .filter(|e| !e.name.contains('/') && e.name.to_lowercase().ends_with(".xml"))
        .collect();
    documents.sort_by(|a, b| a.name.cmp(&b.name));
    if documents.is_empty() {
        return Err(ImportError::new(S::MudletImportNothingFound));
    }
    if documents.len() > MAX_PACKAGE_DOCUMENTS {
        return Err(ImportError::new(S::MudletImportTooManyItems));
    }
    let mut packages = Vec::new();
    for document in documents {
        let xml = archive.read(document, MAX_XML_BYTES).map_err(archive_error)?;
        packages.push(parser::parse(&xml)?);
    }
    Ok(from_packages(&name, packages))
}

/// `mpackage = "Name"` (or with single quotes or long brackets) in a package's config.lua.
fn package_name(config: &str) -> Option<String> {
    let pattern = regex::Regex::new(
        r#"(?m)^\s*mpackage\s*=\s*(?:"([^"\r\n]{1,200})"|'([^'\r\n]{1,200})'|\[\[([^\]\r\n]{1,200})\]\])"#,
    )
    .ok()?;
    let found = pattern.captures(config)?;
    (1..=3)
        .find_map(|i| found.get(i))
        .map(|m| m.as_str().trim().to_string())
}

/// A relative path of plain segments: no root, no drive, no parent or current segment, no
/// backslash, colon or control character, and no device name such as CON or NUL.
pub fn is_safe_entry_name(name: &str) -> bool {
    if name.is_empty() || name.encode_utf16().count() > 512 {
        return false;
    }
    if name
        .chars()
        .any(|c| c.is_control() || matches!(c, '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
    {
        return false;
    }
    if name.starts_with('/') {
        return false;
    }
    for segment in name.trim_end_matches('/').split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return false;
        }
        if segment.ends_with('.') || segment.ends_with(' ') {
            return false;
        }
        let stem = segment.split('.').next().unwrap_or("").to_uppercase();
        let device = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || (stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && stem.as_bytes()[3].is_ascii_digit());
        if device {
            return false;
        }
    }
    true
}
