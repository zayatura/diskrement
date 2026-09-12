//! Catalog/data-dir resolution (`PathResolver.scala`). Precedence:
//! `--catalog`, then `--data-dir`, then `$STATE_DIRECTORY` (first `:`-entry,
//! set-but-empty = unset), then `$XDG_DATA_HOME/diskrement` (empty = unset),
//! then `$HOME/.local/share/diskrement`. The default catalog file name embeds
//! the project name; the fingerprint sidecar is
//! `<catalog-stem>.<project>.scancache.sqlite` beside it.

use std::path::{Path, PathBuf};

pub struct ResolvedPaths {
    pub catalog: PathBuf,
    pub data_dir: PathBuf,
}

/// `toAbsolutePath`: relatives resolve against the cwd, nothing more.
#[allow(dead_code)]
fn absolute(p: &Path) -> PathBuf {
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(p)
    }
}

#[allow(dead_code)]
pub fn resolve(
    catalog: Option<&Path>,
    data_dir: Option<&Path>,
    project: &str,
    get_env: &dyn Fn(&str) -> Option<String>,
    home: &Path,
) -> ResolvedPaths {
    if let Some(c) = catalog {
        let catalog = absolute(c);
        let data_dir = catalog
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        return ResolvedPaths { catalog, data_dir };
    }
    let data_dir = data_dir
        .map(absolute)
        .or_else(|| {
            get_env("STATE_DIRECTORY")
                .filter(|s| !s.is_empty())
                .and_then(|s| s.split(':').next().map(str::to_string))
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
        })
        .or_else(|| {
            get_env("XDG_DATA_HOME")
                .filter(|s| !s.is_empty())
                .map(|s| PathBuf::from(s).join("diskrement"))
        })
        .unwrap_or_else(|| home.join(".local/share/diskrement"));
    ResolvedPaths {
        catalog: data_dir.join(format!("{project}.sqlite")),
        data_dir,
    }
}

#[allow(dead_code)]
pub fn scan_cache_path(catalog: &Path, project: &str) -> PathBuf {
    let name = catalog
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let stem = name.strip_suffix(".sqlite").unwrap_or(&name);
    catalog.with_file_name(format!("{stem}.{project}.scancache.sqlite"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn envmap<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + use<'a> {
        let map: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k: &str| map.get(k).cloned()
    }

    #[test]
    fn defaults_to_home_and_names_the_catalog_after_the_project() {
        let r = resolve(None, None, "photos", &envmap(&[]), Path::new("/home/u"));
        assert_eq!(
            r.catalog,
            PathBuf::from("/home/u/.local/share/diskrement/photos.sqlite")
        );
        assert_eq!(r.data_dir, PathBuf::from("/home/u/.local/share/diskrement"));
    }

    #[test]
    fn xdg_data_home_wins_over_the_default() {
        let r = resolve(
            None,
            None,
            "photos",
            &envmap(&[("XDG_DATA_HOME", "/xdg")]),
            Path::new("/home/u"),
        );
        assert_eq!(r.catalog, PathBuf::from("/xdg/diskrement/photos.sqlite"));
    }

    #[test]
    fn state_directory_beats_xdg_and_takes_the_first_colon_entry() {
        let r = resolve(
            None,
            None,
            "photos",
            &envmap(&[
                ("XDG_DATA_HOME", "/xdg"),
                ("STATE_DIRECTORY", "/state/a:/state/b"),
            ]),
            Path::new("/home/u"),
        );
        assert_eq!(r.data_dir, PathBuf::from("/state/a"));
    }

    #[test]
    fn data_dir_flag_beats_the_environment() {
        let r = resolve(
            None,
            Some(Path::new("/data")),
            "photos",
            &envmap(&[("STATE_DIRECTORY", "/state")]),
            Path::new("/home/u"),
        );
        assert_eq!(r.data_dir, PathBuf::from("/data"));
        assert_eq!(r.catalog, PathBuf::from("/data/photos.sqlite"));
    }

    #[test]
    fn empty_env_values_are_treated_as_unset() {
        let r = resolve(
            None,
            None,
            "photos",
            &envmap(&[("STATE_DIRECTORY", ""), ("XDG_DATA_HOME", "")]),
            Path::new("/home/u"),
        );
        assert_eq!(r.data_dir, PathBuf::from("/home/u/.local/share/diskrement"));
    }

    #[test]
    fn catalog_flag_is_used_directly_and_its_parent_becomes_the_data_dir() {
        let r = resolve(
            Some(Path::new("/data/cat/proj.sqlite")),
            None,
            "p",
            &envmap(&[]),
            Path::new("/home/u"),
        );
        assert_eq!(r.catalog, PathBuf::from("/data/cat/proj.sqlite"));
        assert_eq!(r.data_dir, PathBuf::from("/data/cat"));
    }

    #[test]
    fn a_relative_catalog_flag_is_normalized_against_the_cwd() {
        let cwd = std::env::current_dir().unwrap();
        let r = resolve(
            Some(Path::new("rel/cat.sqlite")),
            None,
            "p",
            &envmap(&[]),
            Path::new("/home/u"),
        );
        assert_eq!(r.catalog, cwd.join("rel/cat.sqlite"));
        assert_eq!(r.data_dir, cwd.join("rel"));
    }

    #[test]
    fn the_scancache_sibling_follows_stem_dot_project() {
        assert_eq!(
            scan_cache_path(Path::new("/data/cat/proj.sqlite"), "p"),
            PathBuf::from("/data/cat/proj.p.scancache.sqlite")
        );
        assert_eq!(
            scan_cache_path(Path::new("/home/u/.local/share/diskrement/p.sqlite"), "p"),
            PathBuf::from("/home/u/.local/share/diskrement/p.p.scancache.sqlite")
        );
        // two projects sharing a catalog file get distinct caches
        assert_ne!(
            scan_cache_path(Path::new("/c/proj.sqlite"), "a"),
            scan_cache_path(Path::new("/c/proj.sqlite"), "b")
        );
    }
}
