//! Named source roots, and the order they imply for filling discs.
//!
//! A project archives one directory (`Sources::Single`) or several, each under
//! a name (`Sources::Rooted`). A rooted project's snapshot holds every root at
//! `snapshot/<name>`, so from the scan onwards its files are ordinary
//! `rel_path`s such as `family/anna/2024/2024-01-15/a.jpg`. Only the planner
//! needs to know roots exist: it fills discs by *(path below the root, root
//! name)*, so the roots interleave instead of following one another
//! (project-roots spec §5).

use std::path::{Path, PathBuf};

use crate::model::RelPath;

/// A root's name: the relative directory its files sit under on every disc.
///
/// One or more `/`-separated components. No component may be empty, `.` or
/// `..`, and the name may not contain NUL or `=` (`=` separates the name from
/// the path in `--root NAME=PATH`). Anything else is allowed, spaces and
/// non-ASCII included.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RootName(String);

impl RootName {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether `self` is `other` or one of its ancestor directories, compared
    /// by whole components: `family` holds `family/anna`, not `familyx`.
    fn holds(&self, other: &RootName) -> bool {
        other.0 == self.0
            || other
                .0
                .strip_prefix(self.0.as_str())
                .is_some_and(|rest| rest.starts_with('/'))
    }
}

impl std::str::FromStr for RootName {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Err("a root name cannot be empty".to_string());
        }
        if s.contains('\0') {
            return Err(format!("root name {s:?} contains NUL"));
        }
        if s.contains('=') {
            return Err(format!(
                "root name {s:?} contains '=', which separates a name from its path"
            ));
        }
        for component in s.split('/') {
            match component {
                "" => {
                    return Err(format!(
                        "root name {s:?} has an empty component \
                         (a leading, trailing or doubled '/')"
                    ))
                }
                "." | ".." => return Err(format!("root name {s:?} has a {component:?} component")),
                _ => {}
            }
        }
        Ok(RootName(s.to_string()))
    }
}

impl std::fmt::Display for RootName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// What a project archives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sources {
    /// One directory, archived as the root of every disc.
    Single(PathBuf),
    /// Named directories, each archived under its name. Sorted by name, and no
    /// name holds another: build it with [`Sources::rooted`].
    Rooted(Vec<(RootName, PathBuf)>),
}

impl Sources {
    /// Named roots, sorted by name. Refuses an empty list, a name given twice,
    /// and a name that holds another (`family` with `family/anna`), whose files
    /// would land inside the other's directory.
    ///
    /// Only the names are checked here. Whether two *paths* overlap depends on
    /// the filesystem, so `Pipeline::init` checks that.
    pub fn rooted(mut roots: Vec<(RootName, PathBuf)>) -> Result<Sources, RootsError> {
        if roots.is_empty() {
            return Err(RootsError::Empty);
        }
        roots.sort_by(|a, b| a.0.cmp(&b.0));
        for (i, (a, _)) in roots.iter().enumerate() {
            for (b, _) in &roots[i + 1..] {
                if a == b {
                    return Err(RootsError::Duplicate(a.clone()));
                }
                // Sorted, so an ancestor always comes before its descendants
                // (a proper prefix sorts first), though not always right
                // before them: `a` < `a-b` < `a/c`.
                if a.holds(b) {
                    return Err(RootsError::Nested {
                        outer: a.clone(),
                        inner: b.clone(),
                    });
                }
            }
        }
        Ok(Sources::Rooted(roots))
    }

    /// Every directory this project archives; in name order when rooted.
    pub fn paths(&self) -> Vec<&Path> {
        match self {
            Sources::Single(path) => vec![path.as_path()],
            Sources::Rooted(roots) => roots.iter().map(|(_, path)| path.as_path()).collect(),
        }
    }
}

/// Why a set of root names was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RootsError {
    Empty,
    Duplicate(RootName),
    Nested { outer: RootName, inner: RootName },
}

impl std::fmt::Display for RootsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RootsError::Empty => f.write_str("a project needs at least one root"),
            RootsError::Duplicate(name) => write!(f, "root name {name} is given twice"),
            RootsError::Nested { outer, inner } => write!(
                f,
                "root {inner} would sit inside root {outer}; root names must not nest"
            ),
        }
    }
}

impl std::error::Error for RootsError {}

/// The order the planner places files in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FillOrder {
    /// A single-source project: by `rel_path`.
    Path,
    /// A rooted project: by the path below the root, then by root name, so the
    /// roots interleave. The names are those of a [`Sources::Rooted`].
    Rooted(Vec<RootName>),
}

impl From<&Sources> for FillOrder {
    fn from(sources: &Sources) -> Self {
        match sources {
            Sources::Single(_) => FillOrder::Path,
            Sources::Rooted(roots) => {
                FillOrder::Rooted(roots.iter().map(|(name, _)| name.clone()).collect())
            }
        }
    }
}

impl FillOrder {
    /// The sort key of `path`: `(path below its root, root name)`. Under
    /// `Path`, and for a path under no root (which a rooted snapshot cannot
    /// produce), it is `(path, "")`.
    ///
    /// No two paths share a key: no name holds another, so a path has at most
    /// one root, and the key spells the path back out. The order is therefore
    /// total, and a plan still depends only on the set of files, not on the
    /// order they arrive in.
    pub fn key<'a>(&self, path: &'a RelPath) -> (&'a str, &'a str) {
        let p = path.as_str();
        match self {
            FillOrder::Path => (p, ""),
            FillOrder::Rooted(names) => names
                .iter()
                .find_map(|name| {
                    let rest = p.strip_prefix(name.as_str())?.strip_prefix('/')?;
                    Some((rest, &p[..name.as_str().len()]))
                })
                .unwrap_or((p, "")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(s: &str) -> RootName {
        s.parse().unwrap()
    }

    fn rel(s: &str) -> RelPath {
        RelPath(s.to_string())
    }

    #[test]
    fn a_root_name_may_have_several_components_spaces_and_accents() {
        for ok in ["anna", "family/anna", "Ilona Dóra", "a.b/c-d_e"] {
            assert_eq!(ok.parse::<RootName>().unwrap().as_str(), ok);
        }
    }

    #[test]
    fn a_root_name_refuses_empty_components_dots_nul_and_equals() {
        for bad in [
            "",
            "/anna",
            "anna/",
            "family//anna",
            ".",
            "..",
            "family/../x",
            "./x",
            "a\0b",
            "a=b",
        ] {
            assert!(bad.parse::<RootName>().is_err(), "{bad:?} was accepted");
        }
    }

    #[test]
    fn rooted_sorts_by_name() {
        let s = Sources::rooted(vec![
            (name("others/cecil"), "/c".into()),
            (name("family/anna"), "/a".into()),
        ])
        .unwrap();
        assert_eq!(
            s,
            Sources::Rooted(vec![
                (name("family/anna"), "/a".into()),
                (name("others/cecil"), "/c".into()),
            ])
        );
    }

    #[test]
    fn rooted_refuses_no_roots_and_a_name_given_twice() {
        assert_eq!(Sources::rooted(vec![]), Err(RootsError::Empty));
        assert_eq!(
            Sources::rooted(vec![
                (name("anna"), "/a".into()),
                (name("anna"), "/b".into())
            ]),
            Err(RootsError::Duplicate(name("anna")))
        );
    }

    #[test]
    fn rooted_refuses_a_name_inside_another_but_not_a_shared_prefix() {
        assert_eq!(
            Sources::rooted(vec![
                (name("family/anna"), "/a".into()),
                (name("family"), "/f".into()),
            ]),
            Err(RootsError::Nested {
                outer: name("family"),
                inner: name("family/anna"),
            })
        );
        // `a` < `a-b` < `a/c`: once sorted, the ancestor is not adjacent.
        assert_eq!(
            Sources::rooted(vec![
                (name("a"), "/1".into()),
                (name("a-b"), "/2".into()),
                (name("a/c"), "/3".into()),
            ]),
            Err(RootsError::Nested {
                outer: name("a"),
                inner: name("a/c"),
            })
        );
        assert!(Sources::rooted(vec![
            (name("family"), "/f".into()),
            (name("familyx"), "/x".into()),
        ])
        .is_ok());
    }

    #[test]
    fn paths_lists_every_directory_in_name_order() {
        let s = Sources::rooted(vec![
            (name("bela"), "/lib/u2".into()),
            (name("anna"), "/lib/u1".into()),
        ])
        .unwrap();
        assert_eq!(s.paths(), vec![Path::new("/lib/u1"), Path::new("/lib/u2")]);
        assert_eq!(
            Sources::Single("/src".into()).paths(),
            vec![Path::new("/src")]
        );
    }

    #[test]
    fn a_single_source_fills_by_path() {
        assert_eq!(
            FillOrder::from(&Sources::Single("/src".into())),
            FillOrder::Path
        );
        assert_eq!(
            FillOrder::Path.key(&rel("family/anna/2021/a.jpg")),
            ("family/anna/2021/a.jpg", "")
        );
    }

    fn three_roots() -> FillOrder {
        FillOrder::from(
            &Sources::rooted(vec![
                (name("family/anna"), "/a".into()),
                (name("family/bela"), "/b".into()),
                (name("cecil"), "/c".into()),
            ])
            .unwrap(),
        )
    }

    #[test]
    fn roots_at_different_depths_interleave_by_the_path_below_them() {
        let order = three_roots();
        let mut paths = vec![
            rel("family/anna/2023/x.jpg"),
            rel("cecil/2022/y.jpg"),
            rel("family/bela/2020/z.jpg"),
            rel("family/anna/2021/w.jpg"),
        ];
        paths.sort_by(|a, b| order.key(a).cmp(&order.key(b)));
        assert_eq!(
            paths,
            vec![
                rel("family/bela/2020/z.jpg"),
                rel("family/anna/2021/w.jpg"),
                rel("cecil/2022/y.jpg"),
                rel("family/anna/2023/x.jpg"),
            ]
        );
    }

    #[test]
    fn the_same_path_below_two_roots_falls_back_to_root_name() {
        let order = three_roots();
        assert_eq!(
            order.key(&rel("family/bela/2020/a.jpg")),
            ("2020/a.jpg", "family/bela")
        );
        assert!(order.key(&rel("cecil/2020/a.jpg")) < order.key(&rel("family/anna/2020/a.jpg")));
    }

    #[test]
    fn a_name_matches_whole_components_only() {
        let order = FillOrder::from(&Sources::rooted(vec![(name("family"), "/f".into())]).unwrap());
        assert_eq!(order.key(&rel("familyx/a.jpg")), ("familyx/a.jpg", ""));
        assert_eq!(order.key(&rel("family/a.jpg")), ("a.jpg", "family"));
    }

    #[test]
    fn a_path_under_no_root_keys_on_the_whole_path() {
        assert_eq!(three_roots().key(&rel("stray.txt")), ("stray.txt", ""));
    }
}
