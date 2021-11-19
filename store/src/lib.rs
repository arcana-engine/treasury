use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use eyre::WrapErr;
use hashbrown::HashMap;
use importer::Importers;
use meta::{AssetMeta, SourceMeta};
use sources::Sources;
use temp::Temporaries;
use treasury_id::{AssetId, AssetIdContext};
use treasury_import::ImportError;
use url::Url;

mod importer;
mod meta;
mod sha256;
mod sources;
mod temp;

const TREASURY_META_NAME: &'static str = "Treasury.toml";
const DEFAULT_AUX: &'static str = "treasury";
const DEFAULT_ARTIFACTS: &'static str = "artifacts";
const DEFAULT_EXTERNAL: &'static str = "external";

#[derive(serde::Serialize, serde::Deserialize)]
struct TreasuryInfo {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    artifacts: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    external: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    temp: Option<PathBuf>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    importers: Vec<PathBuf>,
}

fn write_treasury_info(meta: &TreasuryInfo, path: &Path) -> eyre::Result<()> {
    let meta = toml::to_string_pretty(meta).wrap_err("Failed to serialize metadata")?;
    std::fs::write(path, &meta)
        .wrap_err_with(|| format!("Failed to write metadata file '{}'", path.display()))?;
    Ok(())
}

fn read_treasury_info(path: &Path) -> eyre::Result<TreasuryInfo> {
    let err_ctx = || format!("Failed to read metadata file '{}'", path.display());

    let meta = std::fs::read(path).wrap_err_with(err_ctx)?;
    let meta: TreasuryInfo = toml::from_slice(&meta).wrap_err_with(err_ctx)?;

    Ok(meta)
}

pub struct Treasury {
    id_ctx: AssetIdContext,
    base: PathBuf,
    base_url: Url,
    artifacts: PathBuf,
    external: PathBuf,
    temp: PathBuf,
    importers: Importers,
    artifact_map: HashMap<AssetId, PathBuf>,
}

impl Treasury {
    #[tracing::instrument]
    pub fn init() -> eyre::Result<Self> {
        let cwd = std::env::current_dir().wrap_err("Failed to get current directory")?;
        Treasury::init_in(&cwd, None, None, None, &[])
    }

    #[tracing::instrument]
    pub fn init_in(
        base: &Path,
        artifacts: Option<&Path>,
        external: Option<&Path>,
        temp: Option<&Path>,
        importers: &[&Path],
    ) -> eyre::Result<Self> {
        eyre::ensure!(
            base.is_dir(),
            "Failed to initialize treasury at '{}'. Not a directory",
            base.display()
        );

        let meta_path = base.join(TREASURY_META_NAME);
        eyre::ensure!(
            !meta_path.exists(),
            "Failed to initialize treasury at '{}', '{}' already exists",
            base.display(),
            meta_path.display(),
        );

        let artifacts = artifacts.map(Path::to_owned);
        let external = external.map(Path::to_owned);
        let temp = temp.map(Path::to_owned);
        let importers = importers.iter().copied().map(|p| p.to_owned()).collect();

        let meta = TreasuryInfo {
            artifacts,
            external,
            temp,
            importers,
        };

        write_treasury_info(&meta, &meta_path)?;
        Treasury::open(&meta_path)
    }

    /// Find and open treasury in ancestors of specified directory.
    #[tracing::instrument]
    pub fn find_from(path: &Path) -> eyre::Result<Self> {
        let meta_path = find_treasury_info(path).ok_or_else(|| {
            eyre::eyre!(
                "Failed to find `Treasury.toml` in ancestors of {}",
                path.display(),
            )
        })?;

        Treasury::open(&meta_path)
    }

    /// Find and open treasury in ancestors of current directory.
    #[tracing::instrument]
    pub fn find() -> eyre::Result<Self> {
        let cwd = std::env::current_dir().wrap_err("Failed to get current directory")?;
        Treasury::find_from(&cwd)
    }

    /// Open treasury database at specified path.
    #[tracing::instrument]
    pub fn open(path: &Path) -> eyre::Result<Self> {
        let path = dunce::canonicalize(path).wrap_err_with(|| {
            eyre::eyre!("Failed to canonicalize base path '{}'", path.display())
        })?;

        let meta = read_treasury_info(&path)?;

        let base = path.parent().unwrap().to_owned();
        let base_url = Url::from_directory_path(&base)
            .map_err(|()| eyre::eyre!("'{}' is invalid base path", base.display()))?;

        let artifacts = base.join(
            meta.artifacts
                .unwrap_or_else(|| Path::new(DEFAULT_AUX).join(DEFAULT_ARTIFACTS)),
        );

        let external = base.join(
            meta.external
                .unwrap_or_else(|| Path::new(DEFAULT_AUX).join(DEFAULT_EXTERNAL)),
        );

        let temp = meta
            .temp
            .map_or_else(std::env::temp_dir, |path| base.join(path));

        let id_ctx = AssetIdContext::new(treasury_epoch(), rand::random());

        let mut importers = Importers::new();

        for lib_path in &meta.importers {
            let lib_path = base.join(lib_path);

            unsafe {
                // # Safety: Nope.
                // There is no way to make this safe.
                // But it is unlikely to cause problems by accident.
                if let Err(err) = importers.load_dylib_importers(&lib_path) {
                    tracing::error!(
                        "Failed to load importers from '{}'. {:#}",
                        lib_path.display(),
                        err
                    );
                }
            }
        }

        Ok(Treasury {
            id_ctx,
            base,
            base_url,
            artifacts,
            external,
            temp,
            importers,
            artifact_map: HashMap::new(),
        })
    }

    /// Loads importers from dylib.
    /// There is no possible way to guarantee that dylib does not break safety contracts.
    /// Some measures to ensure safety are taken.
    /// Providing dylib from which importers will be successfully loaded and then cause an UB should possible only on purpose.
    #[tracing::instrument(skip(self))]
    pub unsafe fn register_importers_lib(&mut self, lib_path: &Path) -> eyre::Result<()> {
        self.importers.load_dylib_importers(lib_path)
    }

    /// Import an asset.
    #[tracing::instrument(skip(self))]
    pub async fn store(
        &self,
        source: &str,
        format: Option<&str>,
        target: &str,
    ) -> eyre::Result<AssetId> {
        let source = self.base_url.join(source).wrap_err_with(|| {
            format!(
                "Failed to construct URL from base '{}' and source '{}'",
                self.base_url, source
            )
        })?;

        let mut temporaries = Temporaries::new(&self.temp);
        let mut sources = Sources::new();

        let base = &self.base;
        let artifacts = &self.artifacts;
        let external = &self.external;
        let importers = &self.importers;

        let mut queue = VecDeque::new();
        queue.push_back((source, format.map(str::to_owned), target.to_owned()));

        loop {
            let (source, format, target) = queue.back().unwrap();

            let mut meta = SourceMeta::new(source, &self.base, &self.external)
                .wrap_err("Failed to fetch source meta")?;

            if let Some(asset) = meta.get_asset(target) {
                tracing::debug!(
                    "'{}' '{:?}' '{}' was already imported",
                    source,
                    format,
                    target
                );

                queue.pop_back().unwrap();
                if queue.is_empty() {
                    return Ok(asset.id());
                }
                continue;
            }

            let importer = match format {
                None => importers.guess(url_ext(&source), target)?,
                Some(format) => importers.get(format, target),
            };

            let importer = importer.ok_or_else(|| {
                eyre::eyre!(
                    "Failed to find importer '{} -> {}' for asset '{}'",
                    format.as_deref().unwrap_or("<undefined>"),
                    target,
                    source,
                )
            })?;

            // let format = importer.format();

            // Fetch source file.
            let source_path = sources.fetch(&mut temporaries, source).await?.to_owned();
            let output_path = temporaries.make_temporary();

            let result = importer.import(
                &source_path,
                &output_path,
                |src: &str| {
                    let src = source.join(src).ok()?;
                    // If parsing fails - source will be listed in `ImportResult::RequireSources`
                    // it will fail there again, aborting importing process.
                    // Otherwise it was not important ^_^
                    sources.get(&src)
                },
                |src: &str, target: &str| {
                    let src = source.join(src).ok()?;

                    match SourceMeta::new(&src, base, external) {
                        Ok(meta) => {
                            let asset = meta.get_asset(target)?;
                            Some(asset.id())
                        }
                        Err(err) => {
                            tracing::error!("Fetching dependency failed. {:#}", err);
                            None
                        }
                    }
                },
            );

            match result {
                Ok(()) => {}
                Err(ImportError::Other { reason }) => {
                    return Err(eyre::eyre!(
                        "Failed to import {}:{}->{}. {}",
                        source,
                        importer.format(),
                        target,
                        reason,
                    ))
                }
                Err(ImportError::RequireSources { sources: srcs }) => {
                    let source = source.clone();
                    for src in srcs {
                        match source.join(&src) {
                            Err(err) => {
                                return Err(eyre::eyre!(
                                    "Failed to join URL '{}' with '{}'. {:#}",
                                    source,
                                    src,
                                    err,
                                ))
                            }
                            Ok(url) => {
                                sources.fetch(&mut temporaries, &url).await?;
                            }
                        };
                    }
                    continue;
                }
                Err(ImportError::RequireDependencies { dependencies }) => {
                    let source = source.clone();
                    for dep in dependencies.into_iter() {
                        match source.join(&dep.source) {
                            Err(err) => {
                                return Err(eyre::eyre!(
                                    "Failed to join URL '{}' with '{}'. {:#}",
                                    source,
                                    dep.source,
                                    err,
                                ))
                            }
                            Ok(url) => {
                                queue.push_back((url, None, dep.target));
                            }
                        };
                    }
                    continue;
                }
            }

            if !artifacts.exists() {
                std::fs::create_dir_all(artifacts).wrap_err_with(|| {
                    format!(
                        "Failed to create artifacts directory '{}'",
                        artifacts.display()
                    )
                })?;

                if let Err(err) = std::fs::write(artifacts.join(".gitignore"), "*") {
                    tracing::error!(
                        "Failed to place .gitignore into artifacts directory. {:#}",
                        err
                    );
                }
            }

            let new_id = self.id_ctx.generate();

            let asset = AssetMeta::new(new_id, &output_path, artifacts)
                .wrap_err("Failed to prepare new asset")?;

            let (_url, _format, target) = queue.pop_back().unwrap();

            meta.add_asset(target, asset, base, external)?;

            if queue.is_empty() {
                return Ok(new_id);
            }
        }
    }

    /// Fetch asset data path.
    pub fn fetch(&self, id: AssetId) -> Option<PathBuf> {
        self.artifact_map.get(&id).cloned()
    }

    /// Fetch asset data path.
    pub fn fetch_triple(&self, source: &str, target: &str) -> eyre::Result<Option<PathBuf>> {
        let source = self.base_url.join(source).wrap_err_with(|| {
            format!(
                "Failed to construct URL from base '{}' and source '{}'",
                self.base_url, source
            )
        })?;

        let meta = SourceMeta::new(&source, &self.base, &self.external)
            .wrap_err("Failed to fetch source meta")?;

        match meta.get_asset(target) {
            None => Ok(None),
            Some(asset) => Ok(Some(asset.artifact_path(&self.artifacts))),
        }
    }
}

pub fn find_treasury_info(mut path: &Path) -> Option<PathBuf> {
    loop {
        let candidate = path.join(TREASURY_META_NAME);
        if candidate.is_file() {
            return Some(candidate);
        }
        path = path.parent()?;
    }
}

fn treasury_epoch() -> SystemTime {
    /// Starting point of treasury epoch relative to unix epoch in seconds.
    const TREASURY_EPOCH_FROM_UNIX: u64 = 1609448400;

    SystemTime::UNIX_EPOCH + Duration::from_secs(TREASURY_EPOCH_FROM_UNIX)
}

fn url_ext(url: &Url) -> Option<&str> {
    let path = url.path();
    let dot = path.rfind('.')?;
    if dot == path.len() {
        None
    } else {
        Some(&path[dot + 1..])
    }
}
