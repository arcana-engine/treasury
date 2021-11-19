use std::{
    cell::RefCell,
    error::Error,
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

use eyre::WrapErr;
use hashbrown::{hash_map::Entry, HashMap};
use treasury_id::AssetId;
use url::Url;

use crate::sha256::HashSha256;

const PREFIX_STARTING_LEN: usize = 8;
const EXTENSION: &'static str = "treasure";
const DOT_EXTENSION: &'static str = ".treasure";

/// Data attached to single asset source.
/// It may include several assets.
/// If attached to external source outside treasury directory
/// then it is stored together with artifacts by URL hash.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct SourceMeta {
    url: Url,
    assets: HashMap<String, HashMap<String, AssetMeta>>,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct AssetMeta {
    id: AssetId,
    sha256: HashSha256,

    #[serde(skip_serializing_if = "prefix_is_default", default = "default_prefix")]
    prefix: usize,

    #[serde(skip_serializing_if = "suffix_is_zero", default)]
    suffix: usize,

    #[serde(skip)]
    artifact_path: RefCell<Option<PathBuf>>,
}

fn prefix_is_default(prefix: &usize) -> bool {
    *prefix == PREFIX_STARTING_LEN
}

fn default_prefix() -> usize {
    PREFIX_STARTING_LEN
}

fn suffix_is_zero(suffix: &usize) -> bool {
    *suffix == 0
}

impl AssetMeta {
    pub fn new(id: AssetId, output: &Path, artifacts: &Path) -> eyre::Result<Self> {
        std::fs::create_dir_all(artifacts).wrap_err_with(|| {
            format!(
                "Failed to create artifacts directory '{}'",
                artifacts.display()
            )
        })?;

        let sha256 = HashSha256::file_hash(output).wrap_err_with(|| {
            format!(
                "Failed to calculate hash of the file '{}'",
                output.display()
            )
        })?;

        let hex = format!("{:x}", sha256);

        with_path_candidates(&hex, artifacts, move |prefix, suffix, path| {
            match path.metadata() {
                Err(_) => {
                    // This is the most common case.
                    std::fs::rename(output, &path).wrap_err_with(|| {
                        format!(
                            "Failed to rename output file '{}' to artifact file '{}'",
                            output.display(),
                            path.display()
                        )
                    })?;

                    Ok(Some(AssetMeta {
                        id,
                        sha256,
                        prefix,
                        suffix,
                        artifact_path: RefCell::new(Some(path)),
                    }))
                }
                Ok(meta) if meta.is_file() => {
                    let eq = files_eq(output, &path).wrap_err_with(|| {
                        format!(
                            "Failed to compare artifact file '{}' and new asset output '{}'",
                            path.display(),
                            output.display(),
                        )
                    })?;

                    if eq {
                        tracing::warn!("Artifact for asset '{}' is already in storage", id);

                        if let Err(err) = std::fs::remove_file(output) {
                            tracing::error!(
                                "Failed to remove duplicate artifact file '{}'. {:#}",
                                err,
                                output.display()
                            );
                        }

                        Ok(Some(AssetMeta {
                            id,
                            sha256,
                            prefix,
                            suffix,
                            artifact_path: RefCell::new(Some(path)),
                        }))
                    } else {
                        tracing::debug!("Artifact path collision");
                        Ok(None)
                    }
                }
                Ok(_) => {
                    tracing::warn!(
                        "Artifacts storage occupied by non-file entity '{}'",
                        path.display()
                    );
                    Ok(None)
                }
            }
        })
    }

    pub fn id(&self) -> AssetId {
        self.id
    }

    // pub fn hash(&self) -> &HashSha256 {
    //     &self.sha256
    // }

    pub fn artifact_path(&self, artifacts: &Path) -> PathBuf {
        let mut artifact_path = self.artifact_path.borrow_mut();

        if artifact_path.is_none() {
            let path = self.make_artifact_path(artifacts);
            *artifact_path = Some(path);
        } else {
            debug_assert_eq!(
                artifact_path.as_ref(),
                Some(&self.make_artifact_path(artifacts))
            );
        }
        artifact_path.as_ref().unwrap().to_owned()
    }

    fn make_artifact_path(&self, artifacts: &Path) -> PathBuf {
        let hex = format!("{:x}", self.sha256);
        let prefix = &hex[..self.prefix as usize];

        match self.suffix {
            0 => artifacts.join(prefix),
            suffix => artifacts.join(format!("{}:{}", prefix, suffix)),
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("Error: '{}' while trying to canonicalize path '{}'", error, path.display())]
struct CanonError {
    #[source]
    error: std::io::Error,
    path: PathBuf,
}

#[derive(Debug, thiserror::Error)]
#[error("Failed to convert path '{}' to URL", path.display())]
struct UrlFromPathError {
    path: PathBuf,
}

#[derive(Debug, thiserror::Error)]
#[error("Error: '{}' with file: '{}'", error, path.display())]
struct FileError<E: Error> {
    #[source]
    error: E,
    path: PathBuf,
}

impl SourceMeta {
    /// Finds and returns meta for the source URL.
    /// Creates new file if needed.
    pub fn new(source: &Url, base: &Path, external: &Path) -> eyre::Result<SourceMeta> {
        let (meta_path, _is_external) = get_meta_path(source, base, external)?;
        Self::open(&meta_path, base, external)
    }

    pub fn open(meta_path: &Path, base: &Path, external: &Path) -> eyre::Result<SourceMeta> {
        let meta_path = dunce::canonicalize(meta_path).map_err(|err| CanonError {
            error: err,
            path: meta_path.to_owned(),
        })?;

        if meta_path.extension() == Some(EXTENSION.as_ref()) {
            // Local case.
            if !meta_path.starts_with(base) {
                return Err(eyre::eyre!(
                    "Local meta path '{}' is expected to be prefixed by base path '{}'",
                    meta_path.display(),
                    base.display(),
                ));
            }
            let source_path = meta_path.with_extension("");
            let url = Url::from_file_path(&source_path)
                .map_err(|()| UrlFromPathError { path: source_path })?;
            let meta = SourceMeta::read_from(url, &meta_path)?;
            Ok(meta)
        } else {
            // External case
            if !meta_path.starts_with(external) {
                return Err(eyre::eyre!(
                    "External meta path '{}' is expected to be prefixed by base path '{}'",
                    meta_path.display(),
                    external.display(),
                ));
            }

            if meta_path.extension().is_some() {
                return Err(eyre::eyre!(
                    "External meta path '{}' must have no extension",
                    meta_path.display(),
                ));
            }

            let meta = SourceMeta::read_with_url_from(&meta_path)?;

            Ok(meta)
        }
    }

    pub fn get_asset(&self, format: &str, target: &str) -> Option<&AssetMeta> {
        self.assets.get(format)?.get(target)
    }

    pub fn add_asset(
        &mut self,
        format: String,
        target: String,
        asset: AssetMeta,
        base: &Path,
        external: &Path,
    ) -> eyre::Result<()> {
        match self.assets.entry(format) {
            Entry::Vacant(entry) => {
                entry.insert(HashMap::new()).insert(target, asset);
            }
            Entry::Occupied(mut entry) => match entry.get_mut().entry(target) {
                Entry::Vacant(entry) => {
                    entry.insert(asset);
                }
                Entry::Occupied(_) => {
                    panic!("Asset already exists");
                }
            },
        }

        let (meta_path, is_external) = get_meta_path(&self.url, base, external)?;
        if is_external {
            self.write_with_url_to(&meta_path)?;
        } else {
            self.write_to(&meta_path)?;
        }
        Ok(())
    }

    fn write_to(&self, path: &Path) -> eyre::Result<()> {
        let data = toml::to_string_pretty(&self.assets)
            .map_err(|err| FileError {
                error: err,
                path: path.to_owned(),
            })
            .wrap_err("Meta write failed")?;
        std::fs::write(path, data.as_bytes())
            .map_err(|err| FileError {
                error: err,
                path: path.to_owned(),
            })
            .wrap_err("Meta write failed")?;
        Ok(())
    }

    fn write_with_url_to(&self, path: &Path) -> eyre::Result<()> {
        let data = toml::to_string_pretty(self)
            .map_err(|err| FileError {
                error: err,
                path: path.to_owned(),
            })
            .wrap_err("Meta write failed")?;
        std::fs::write(path, data.as_bytes())
            .map_err(|err| FileError {
                error: err,
                path: path.to_owned(),
            })
            .wrap_err("Meta write failed")?;
        Ok(())
    }

    fn read_from(url: Url, path: &Path) -> eyre::Result<Self> {
        let data = std::fs::read(path)
            .map_err(|err| FileError {
                error: err,
                path: path.to_owned(),
            })
            .wrap_err("Meta read failed")?;
        let assets = toml::from_slice(&data)
            .map_err(|err| FileError {
                error: err,
                path: path.to_owned(),
            })
            .wrap_err("Meta read failed")?;
        Ok(SourceMeta { url, assets })
    }

    fn read_with_url_from(path: &Path) -> eyre::Result<Self> {
        let data = std::fs::read(path)
            .map_err(|err| FileError {
                error: err,
                path: path.to_owned(),
            })
            .wrap_err("Meta read failed")?;
        let meta = toml::from_slice(&data)
            .map_err(|err| FileError {
                error: err,
                path: path.to_owned(),
            })
            .wrap_err("Meta read failed")?;
        Ok(meta)
    }
}

fn files_eq(lhs: &Path, rhs: &Path) -> std::io::Result<bool> {
    let mut lhs = File::open(lhs)?;
    let mut rhs = File::open(rhs)?;

    let lhs_size = lhs.seek(SeekFrom::End(0))?;
    let rhs_size = rhs.seek(SeekFrom::End(0))?;

    if lhs_size != rhs_size {
        return Ok(false);
    }

    lhs.seek(SeekFrom::Start(0))?;
    rhs.seek(SeekFrom::Start(0))?;

    let mut buffer_lhs = [0; 16536];
    let mut buffer_rhs = [0; 16536];

    loop {
        let read = lhs.read(&mut buffer_lhs)?;
        if read == 0 {
            return Ok(true);
        }
        rhs.read_exact(&mut buffer_rhs[..read])?;

        if buffer_lhs[..read] != buffer_rhs[..read] {
            return Ok(false);
        }
    }
}

/// Finds and returns meta for the source URL.
/// Creates new file if needed.
fn get_meta_path(source: &Url, base: &Path, external: &Path) -> eyre::Result<(PathBuf, bool)> {
    if source.scheme() == "file" {
        match source.to_file_path() {
            Ok(path) => {
                let path =
                    dunce::canonicalize(&path).map_err(|err| CanonError { error: err, path })?;

                if path.starts_with(base) {
                    // Files inside `base` directory has meta attached to them as sibling file with `.treasure` extension added.

                    let mut filename = path.file_name().unwrap_or("".as_ref()).to_owned();
                    filename.push(DOT_EXTENSION);

                    let path = path.with_file_name(filename);

                    if !path.exists() {
                        let meta = SourceMeta {
                            url: source.clone(),
                            assets: HashMap::new(),
                        };
                        meta.write_to(&path)?;
                    }
                    return Ok((path, false));
                }
            }
            Err(()) => {}
        }
    }

    std::fs::create_dir_all(external).wrap_err_with(|| {
        format!(
            "Failed to create external directory '{}'",
            external.display()
        )
    })?;

    let hash = HashSha256::new(source.as_str());
    let hex = format!("{:x}", hash);

    with_path_candidates(&hex, external, |_prefix, _suffix, path| {
        match path.metadata() {
            Err(_) => {
                // Not exists. Let's try to occupy.

                let meta = SourceMeta {
                    url: source.clone(),
                    assets: HashMap::new(),
                };

                meta.write_with_url_to(&path)
                    .wrap_err("Failed to save new source meta")?;

                Ok(Some((path, true)))
            }
            Ok(md) => {
                if md.is_file() {
                    match SourceMeta::read_with_url_from(&path) {
                        Err(_) => {
                            tracing::error!(
                                "Failed to open existing source metadata at '{}'",
                                path.display()
                            );
                        }
                        Ok(meta) => {
                            if meta.url == *source {
                                return Ok(Some((path, true)));
                            }
                        }
                    }
                }
                Ok(None)
            }
        }
    })
}

fn with_path_candidates<T, E>(
    hex: &str,
    base: &Path,
    mut f: impl FnMut(usize, usize, PathBuf) -> Result<Option<T>, E>,
) -> Result<T, E> {
    for len in PREFIX_STARTING_LEN..=hex.len() {
        let name = &hex[..len];
        let path = base.join(name);

        match f(len, 0, path) {
            Ok(None) => {}
            Ok(Some(ok)) => return Ok(ok),
            Err(err) => return Err(err),
        }
    }

    for suffix in 0usize.. {
        let name = format!("{}:{}", hex, suffix);
        let path = base.join(name);

        match f(hex.len(), suffix, path) {
            Ok(None) => {}
            Ok(Some(ok)) => return Ok(ok),
            Err(err) => return Err(err),
        }
    }

    unreachable!()
}
