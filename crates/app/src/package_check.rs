//! Finds missing and unused LaTeX packages in a whole project and places the
//! result in the files, for the underlines and the list in the log panel.

use std::{
    ops::Range,
    path::{Path, PathBuf},
};

use build::{Fix, Loaded, PackageFinding, ProjectUsage, check, loaded};
use editor_core::latex_usage::{LatexUsage, latex_usage};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageHint {
    pub path: PathBuf,
    pub range: Range<usize>,
    pub message: String,
    pub fix: Fix,
}

/// What one check finds, for the hints and for the suggestions.
pub struct Report {
    pub hints: Vec<PackageHint>,
    pub loaded: Loaded,
    /// Every package the project loads, by name.
    pub packages: Vec<String>,
    /// Every command the project uses, sorted.
    pub used: Vec<String>,
}

/// `sources` holds every LaTeX file of the project. `main` gets a new package.
pub fn hints(main: &Path, sources: &[(PathBuf, String)], has_own_style_files: bool) -> Report {
    let files: Vec<(&Path, LatexUsage)> = sources.iter().map(|(path, text)| (path.as_path(), latex_usage(text))).collect();
    let mut project = ProjectUsage { has_own_style_files, ..Default::default() };
    for (_, usage) in &files {
        project.classes.extend(usage.class.clone());
        project.packages.extend(usage.packages.iter().map(|package| package.name.clone()));
        project.commands.extend(usage.commands.keys().cloned());
        project.environments.extend(usage.environments.keys().cloned());
        project.defined.extend(usage.defined.iter().cloned());
    }
    // A new package needs a preamble to go to.
    let main_takes_packages = files.iter().any(|(path, usage)| *path == main && usage.class.is_some());
    let hints = check(&project)
        .into_iter()
        .filter_map(|finding| match finding {
            PackageFinding::Missing { package, item } => {
                if !main_takes_packages {
                    return None;
                }
                let (path, range) = files.iter().find_map(|(path, usage)| {
                    usage.commands.get(&item).or_else(|| usage.environments.get(&item)).map(|range| (*path, range.clone()))
                })?;
                Some(PackageHint {
                    path: path.to_path_buf(),
                    range,
                    message: format!("{item} needs {package}, which is not loaded"),
                    fix: Fix::AddPackage(package.to_string()),
                })
            }
            PackageFinding::Unused { package } => {
                let (path, range) = files.iter().find_map(|(path, usage)| {
                    let loaded = usage.packages.iter().find(|loaded| loaded.name == package)?;
                    Some((*path, loaded.name_range.clone()))
                })?;
                Some(PackageHint {
                    path: path.to_path_buf(),
                    range,
                    message: format!("{package} seems unused"),
                    fix: Fix::RemovePackage(package),
                })
            }
        })
        .collect();
    let mut packages = project.packages.clone();
    packages.sort();
    packages.dedup();
    let mut used: Vec<String> = project.commands.iter().cloned().collect();
    used.sort();
    Report { hints, loaded: loaded(&project), packages, used }
}

/// The edit that `fix` makes in `text`, or `None` when the file has no place for it.
pub fn edit_for(fix: &Fix, text: &str) -> Option<(Range<usize>, String)> {
    let usage = latex_usage(text);
    match fix {
        Fix::AddPackage(package) => {
            if usage.packages.iter().any(|loaded| loaded.name == *package) {
                return None;
            }
            let (offset, line) = usage.package_insertion(text, package)?;
            Some((offset..offset, line))
        }
        Fix::RemovePackage(package) => Some((usage.package_removal(text, package)?, String::new())),
        Fix::InstallTool(_) | Fix::InstallLatexPackage(_) | Fix::UpdateLatex => None,
    }
}

/// Colors for `lstlisting`, close to what code editors show; plain `listings`
/// only sets keywords in bold.
const LISTINGS_STYLE: &str = "\\lstset{
  basicstyle=\\ttfamily\\small\\color[HTML]{1F3A68},
  keywordstyle=\\color[HTML]{871094}\\bfseries,
  commentstyle=\\color[HTML]{8C8C8C}\\itshape,
  stringstyle=\\color[HTML]{067D17},
  numberstyle=\\tiny\\color[HTML]{999999},
  numbers=left,
  frame=single,
  rulecolor=\\color[HTML]{DDDDDD},
  backgroundcolor=\\color[HTML]{F7F7F7},
  breaklines=true,
  showstringspaces=false,
  tabsize=4,
  columns=fullflexible,
  keepspaces=true,
}";

/// The edit that loads `listings` and `xcolor` and styles code blocks, or
/// `None` when the file has no preamble or styles them already.
pub fn listings_setup(text: &str) -> Option<(Range<usize>, String)> {
    let usage = latex_usage(text);
    let (offset, _) = usage.package_insertion(text, "listings")?;
    let mut lines = String::new();
    for package in ["listings", "xcolor"] {
        if !usage.packages.iter().any(|loaded| loaded.name == package) {
            lines.push_str(&format!("\n\\usepackage{{{package}}}"));
        }
    }
    if !text.contains("\\lstset") && !text.contains("\\lstdefinestyle") {
        lines.push('\n');
        lines.push_str(LISTINGS_STYLE);
    }
    (!lines.is_empty()).then_some((offset..offset, lines))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAIN: &str = "\\documentclass{article}\n\\usepackage{booktabs}\n\\begin{document}\n\\input{part}\n\\end{document}\n";

    fn files(entries: &[(&str, &str)]) -> Vec<(PathBuf, String)> {
        entries.iter().map(|(path, text)| (PathBuf::from(path), text.to_string())).collect()
    }

    #[test]
    fn use_in_another_file_keeps_a_package() {
        let sources = files(&[("main.tex", MAIN), ("part.tex", "\\toprule\n")]);
        assert_eq!(hints(Path::new("main.tex"), &sources, false).hints, []);
    }

    #[test]
    fn an_unused_package_is_marked_where_it_is_loaded() {
        let sources = files(&[("main.tex", MAIN), ("part.tex", "text\n")]);
        let found = hints(Path::new("main.tex"), &sources, false).hints;
        assert_eq!(found.len(), 1);
        assert_eq!(&MAIN[found[0].range.clone()], "booktabs");
        assert_eq!(found[0].fix, Fix::RemovePackage("booktabs".to_string()));
    }

    #[test]
    fn a_missing_package_is_marked_at_the_first_use_in_any_file() {
        let main = "\\documentclass{article}\n\\begin{document}\n\\input{part}\n\\end{document}\n";
        let part = "a \\toprule\n";
        let sources = files(&[("main.tex", main), ("part.tex", part)]);
        let found = hints(Path::new("main.tex"), &sources, false).hints;
        assert_eq!(found[0].path, Path::new("part.tex"));
        assert_eq!(&part[found[0].range.clone()], "\\toprule");
        assert_eq!(found[0].fix, Fix::AddPackage("booktabs".to_string()));
    }

    #[test]
    fn a_main_without_a_preamble_gets_no_add_hint() {
        let sources = files(&[("root.tex", "\\documentclass{article}\n"), ("main.tex", "\\toprule\n")]);
        assert_eq!(hints(Path::new("main.tex"), &sources, false).hints, []);
    }

    #[test]
    fn the_shipped_sample_gets_no_hint() {
        let text = include_str!("../../../examples/overleaf-sample/main.tex");
        let sources = files(&[("main.tex", text)]);
        assert_eq!(hints(Path::new("main.tex"), &sources, false).hints, []);
    }

    #[test]
    fn listings_get_set_up_once() {
        let (range, lines) = listings_setup(MAIN).unwrap();
        assert!(lines.starts_with("\n\\usepackage{listings}\n\\usepackage{xcolor}\n\\lstset{"));
        let mut text = MAIN.to_string();
        text.replace_range(range, &lines);
        assert_eq!(listings_setup(&text), None);
        assert_eq!(listings_setup("\\section{Part}\n"), None);
    }

    /// Needs `pdflatex`: `cargo test -p app -- --ignored`.
    #[test]
    #[ignore]
    fn the_listings_style_compiles() {
        let directory = tempfile::tempdir().unwrap();
        let main = "\\documentclass{article}\n\\begin{document}\n\\begin{lstlisting}[language=Python]\nprint(\"Hello\") # hi\n\\end{lstlisting}\n\\end{document}\n";
        let (range, lines) = listings_setup(main).unwrap();
        let mut text = main.to_string();
        text.replace_range(range, &lines);
        std::fs::write(directory.path().join("main.tex"), text).unwrap();
        let output = std::process::Command::new(build::find_tool("pdflatex").expect("LaTeX is installed"))
            .args(["-interaction=nonstopmode", "-halt-on-error", "main.tex"])
            .current_dir(directory.path())
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stdout));
    }

    #[test]
    fn adding_twice_does_nothing() {
        assert_eq!(edit_for(&Fix::AddPackage("booktabs".to_string()), MAIN), None);
    }
}
