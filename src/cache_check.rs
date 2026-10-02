//! `coce cache-check`: compare the local image copies (providers with
//! `cache: true`) with the URLs stored in Redis, and optionally repair
//! Redis (and remove broken files) from what is actually on disk.

use crate::config::Config;
use crate::isbn;
use crate::redis_store::{self, RedisManager};
use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::path::{Path, PathBuf};

/// Options of `coce cache-check` (described on `Command::CacheCheck`).
#[derive(clap::Args)]
#[command(after_help = "Exit code: 0 if nothing needs fixing (or --fix was given), \
1 if fixes are needed, 2 on usage error.")]
pub struct Args {
    /// Check this provider only
    #[arg(long)]
    pub provider: Option<String>,
    /// Apply the fixes: delete keys pointing to missing or broken files,
    /// delete broken files, point keys to the local file when one exists
    #[arg(long)]
    pub fix: bool,
    /// Also recreate missing keys for files on disk (beware: files keep the
    /// imageSize they were downloaded with)
    #[arg(long, requires = "fix")]
    pub restore: bool,
    /// List the IDs of each case
    #[arg(short, long)]
    pub verbose: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Case {
    Ok,
    MissingFile,
    InvalidFile,
    RemoteWithFile,
    OldBaseUrl,
    FileWithoutKey,
    Forced,
    NotFoundWithFile,
    NonCanonicalName,
}

impl Case {
    fn label(self) -> &'static str {
        match self {
            Case::Ok => "consistent",
            Case::MissingFile => "key -> missing file",
            Case::InvalidFile => "broken file (not an image)",
            Case::RemoteWithFile => "remote URL, file on disk",
            Case::OldBaseUrl => "local URL, old cache.url",
            Case::FileWithoutKey => "file without key",
            Case::Forced => "URL forced by /set, file on disk",
            Case::NotFoundWithFile => "\"no cover\" key, file on disk",
            Case::NonCanonicalName => "file not named by ISBN-13",
        }
    }

    fn fix(self, restore: bool) -> &'static str {
        match self {
            Case::Ok => "",
            Case::MissingFile => "delete key",
            Case::InvalidFile => "delete file (and key if local)",
            Case::RemoteWithFile | Case::OldBaseUrl => "point key to local file",
            Case::FileWithoutKey if restore => "recreate key",
            Case::FileWithoutKey => "left as is (see --restore)",
            Case::Forced | Case::NotFoundWithFile | Case::NonCanonicalName => "reported only",
        }
    }

    /// Whether `--fix` changes something for this case.
    fn acts(self, restore: bool) -> bool {
        self.needs_fix() || (self == Case::FileWithoutKey && restore)
    }

    /// Whether this case means Redis serves something wrong, which makes the
    /// report-only run exit with 1.
    fn needs_fix(self) -> bool {
        matches!(
            self,
            Case::MissingFile | Case::InvalidFile | Case::RemoteWithFile | Case::OldBaseUrl
        )
    }
}

#[derive(Clone, Copy, PartialEq)]
enum FileState {
    Missing,
    Valid,
    Invalid,
}

enum KeyState {
    Absent,
    NotFound,
    Local,
    OldLocal,
    Remote,
}

/// What `--fix` does, collected over a provider before being applied.
#[derive(Default)]
struct Fixes {
    delete_keys: Vec<String>,
    set_keys: Vec<(String, String, u64)>,
    delete_files: Vec<PathBuf>,
}

/// Run the command; returns the process exit code.
pub async fn run(cfg: &Config, redis: &mut RedisManager, opts: &Args) -> anyhow::Result<i32> {
    let Some(cache_cfg) = cfg.cache.as_ref() else {
        eprintln!("No local image cache configured (`cache.path` / `cache.url`).");
        return Ok(2);
    };
    let cached: Vec<&String> = cfg
        .providers
        .iter()
        .filter(|p| cfg.provider_config(p).is_some_and(|c| c.cache))
        .filter(|p| opts.provider.as_ref().is_none_or(|only| only == *p))
        .collect();
    if cached.is_empty() {
        eprintln!("No provider with `cache: true` to check.");
        return Ok(2);
    }

    let mut needs_fix = false;
    for provider in cached {
        let dir = Path::new(&cache_cfg.path).join(provider);
        let local_prefix = format!("{}/{}/", cache_cfg.url, provider);
        let found_ttl = cfg.found_ttl(provider);

        let files = scan_dir(&dir)?;
        let keys = scan_keys(redis, provider).await?;

        let mut ids: Vec<&String> = files.keys().chain(keys.keys()).collect();
        ids.sort();
        ids.dedup();

        let mut cases: BTreeMap<Case, Vec<String>> = BTreeMap::new();
        let mut fixes = Fixes::default();
        for id in ids {
            let file = files.get(id).copied().unwrap_or(FileState::Missing);
            let key = format!("{provider}.{id}");
            let path = dir.join(format!("{id}.jpg"));
            let local_url = format!("{local_prefix}{id}.jpg");
            let key_state = match keys.get(id) {
                None => KeyState::Absent,
                Some(v) if v.is_empty() => KeyState::NotFound,
                Some(v) if *v == local_url => KeyState::Local,
                Some(v) if v.ends_with(&format!("/{provider}/{id}.jpg")) => KeyState::OldLocal,
                Some(_) => KeyState::Remote,
            };

            let case = match (&key_state, file) {
                _ if file != FileState::Missing && isbn::canonical(id) != *id => Case::NonCanonicalName,
                (_, FileState::Invalid) => {
                    fixes.delete_files.push(path);
                    if matches!(key_state, KeyState::Local | KeyState::OldLocal) {
                        fixes.delete_keys.push(key);
                    }
                    Case::InvalidFile
                }
                (KeyState::Local | KeyState::OldLocal, FileState::Missing) => {
                    fixes.delete_keys.push(key);
                    Case::MissingFile
                }
                (KeyState::OldLocal, FileState::Valid) => {
                    fixes.set_keys.push((key, local_url, found_ttl));
                    Case::OldBaseUrl
                }
                (KeyState::Remote, FileState::Valid) => {
                    // /set stores its URL for 10 years: never override a
                    // cover forced by hand with the provider's image.
                    if redis_store::ttl(redis, &key).await? > found_ttl as i64 {
                        Case::Forced
                    } else {
                        fixes.set_keys.push((key, local_url, found_ttl));
                        Case::RemoteWithFile
                    }
                }
                (KeyState::Absent, FileState::Valid) => {
                    if opts.restore {
                        fixes.set_keys.push((key, local_url, found_ttl));
                    }
                    Case::FileWithoutKey
                }
                (KeyState::NotFound, FileState::Valid) => Case::NotFoundWithFile,
                _ => Case::Ok,
            };
            cases.entry(case).or_default().push(id.clone());
        }

        let valid = files.values().filter(|f| **f == FileState::Valid).count();
        let not_found = keys.values().filter(|v| v.is_empty()).count();
        println!("{provider}  ({})", dir.display());
        println!(
            "  files on disk: {} ({valid} valid)    keys in Redis: {} ({not_found} \"no cover\")",
            files.len(),
            keys.len()
        );
        for (case, ids) in &cases {
            let action = if !opts.fix && case.acts(opts.restore) {
                format!("would {}", case.fix(opts.restore))
            } else {
                case.fix(opts.restore).to_string()
            };
            println!("  {:<36} {:>7}   {action}", case.label(), ids.len());
            if opts.verbose && *case != Case::Ok {
                for id in ids {
                    println!("      {id}");
                }
            }
            needs_fix |= case.needs_fix();
        }

        if opts.fix {
            apply(redis, &fixes).await?;
            println!(
                "  applied: {} key(s) deleted, {} key(s) set, {} file(s) deleted",
                fixes.delete_keys.len(),
                fixes.set_keys.len(),
                fixes.delete_files.len()
            );
        }
        println!();
    }

    Ok(if needs_fix && !opts.fix { 1 } else { 0 })
}

/// `<id>` -> state of `<dir>/<id>.jpg`. A missing directory is just empty.
fn scan_dir(dir: &Path) -> anyhow::Result<HashMap<String, FileState>> {
    let mut files = HashMap::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(files),
        Err(e) => return Err(e.into()),
    };
    for entry in entries {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) != Some("jpg") {
            continue;
        }
        if let Some(id) = path.file_stem().and_then(|s| s.to_str()) {
            let state = if is_image(&path) {
                FileState::Valid
            } else {
                FileState::Invalid
            };
            files.insert(id.to_string(), state);
        }
    }
    Ok(files)
}

/// Whether the file starts like an image (JPEG, PNG, GIF or WebP), rather
/// than being empty, truncated before its header, or an error page.
fn is_image(path: &Path) -> bool {
    let mut head = Vec::with_capacity(12);
    let read = std::fs::File::open(path).and_then(|f| f.take(12).read_to_end(&mut head));
    if read.is_err() {
        return false;
    }
    head.starts_with(&[0xFF, 0xD8, 0xFF])
        || head.starts_with(b"\x89PNG")
        || head.starts_with(b"GIF8")
        || (head.len() == 12 && &head[..4] == b"RIFF" && &head[8..12] == b"WEBP")
}

/// `<id>` -> value of every `<provider>.<id>` key.
async fn scan_keys(redis: &mut RedisManager, provider: &str) -> anyhow::Result<HashMap<String, String>> {
    let prefix = format!("{provider}.");
    let keys = redis_store::scan(redis, &format!("{prefix}*")).await?;
    let mut values = HashMap::with_capacity(keys.len());
    for chunk in keys.chunks(1000) {
        for (key, value) in chunk.iter().zip(redis_store::get_many(redis, chunk).await?) {
            // A key may expire between the scan and the read.
            if let (Some(id), Some(value)) = (key.strip_prefix(&prefix), value) {
                values.insert(id.to_string(), value);
            }
        }
    }
    Ok(values)
}

async fn apply(redis: &mut RedisManager, fixes: &Fixes) -> anyhow::Result<()> {
    for path in &fixes.delete_files {
        if let Err(e) = std::fs::remove_file(path) {
            eprintln!("cannot delete {}: {e}", path.display());
        }
    }
    for chunk in fixes.delete_keys.chunks(1000) {
        redis_store::del(redis, chunk).await?;
    }
    for chunk in fixes.set_keys.chunks(1000) {
        redis_store::set_many_ex(redis, chunk).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_images() {
        let dir = std::env::temp_dir().join(format!("coce-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cases: &[(&str, &[u8], bool)] = &[
            ("jpeg", &[0xFF, 0xD8, 0xFF, 0xE0, 0, 0x10], true),
            ("png", b"\x89PNG\r\n\x1a\n", true),
            ("gif", b"GIF89a", true),
            ("webp", b"RIFF\0\0\0\0WEBPVP8 ", true),
            ("html", b"<!DOCTYPE html><html>", false),
            ("empty", b"", false),
        ];
        for (name, bytes, expected) in cases {
            let path = dir.join(format!("{name}.jpg"));
            std::fs::write(&path, bytes).unwrap();
            assert_eq!(is_image(&path), *expected, "{name}");
        }
        assert!(!is_image(&dir.join("absent.jpg")));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
