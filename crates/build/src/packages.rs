//! Which LaTeX package provides what. A wrong hint costs more trust than a missing
//! one helps, so `check` stays silent whenever it cannot be sure. The compiler's
//! error remains the safety net.

use std::{
    collections::{HashMap, HashSet},
    sync::OnceLock,
};

use crate::explain::edit_distance;

/// What a click can do about a package.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Fix {
    AddPackage(String),
    RemovePackage(String),
}

impl Fix {
    pub fn label(&self) -> String {
        match self {
            Self::AddPackage(package) => format!("Add \\usepackage{{{package}}}"),
            Self::RemovePackage(package) => format!("Remove {package}"),
        }
    }
}

struct Package {
    name: &'static str,
    commands: &'static [&'static str],
    environments: &'static [&'static str],
    /// Packages that this one loads.
    implies: &'static [&'static str],
    /// All its commands are listed, and loading it alone changes nothing.
    checkable: bool,
}

const fn package(
    name: &'static str,
    commands: &'static [&'static str],
    environments: &'static [&'static str],
    implies: &'static [&'static str],
    checkable: bool,
) -> Package {
    Package { name, commands, environments, implies, checkable }
}

/// The first package that lists a command is the one to suggest.
const PACKAGES: &[Package] = &[
    package(
        "amsmath",
        &["\\text", "\\eqref", "\\DeclareMathOperator", "\\dfrac", "\\tfrac", "\\binom", "\\boxed", "\\intertext", "\\numberwithin", "\\substack", "\\tag", "\\overset", "\\underset", "\\operatorname", "\\iint", "\\iiint"],
        &["align", "align*", "gather", "gather*", "multline", "multline*", "flalign", "flalign*", "alignat", "alignat*", "split", "aligned", "gathered", "cases", "matrix", "pmatrix", "bmatrix", "vmatrix", "Vmatrix", "Bmatrix", "subequations"],
        &[],
        false,
    ),
    package("amssymb", &["\\mathbb", "\\mathfrak", "\\therefore", "\\because", "\\varnothing", "\\leqslant", "\\geqslant", "\\square", "\\blacksquare", "\\checkmark", "\\nexists", "\\lesssim", "\\gtrsim"], &[], &["amsfonts"], false),
    package("amsfonts", &["\\mathbb", "\\mathfrak"], &[], &[], false),
    package("amsthm", &["\\theoremstyle", "\\qedhere", "\\qed"], &["proof"], &[], false),
    package("mathtools", &["\\coloneqq", "\\mathclap", "\\DeclarePairedDelimiter"], &[], &["amsmath"], false),
    package("graphicx", &["\\includegraphics", "\\graphicspath", "\\rotatebox", "\\scalebox", "\\resizebox", "\\reflectbox"], &[], &["graphics"], true),
    package("graphics", &["\\includegraphics", "\\graphicspath", "\\rotatebox", "\\scalebox", "\\resizebox", "\\reflectbox"], &[], &[], false),
    package("hyperref", &["\\href", "\\url", "\\hyperref", "\\hypersetup", "\\nolinkurl", "\\texorpdfstring", "\\autoref", "\\hyperlink", "\\hypertarget"], &[], &["url"], false),
    package("url", &["\\url", "\\urlstyle"], &[], &[], false),
    package("xcolor", &["\\textcolor", "\\color", "\\colorbox", "\\fcolorbox", "\\definecolor", "\\pagecolor"], &[], &["color"], false),
    package("color", &["\\textcolor", "\\color", "\\colorbox", "\\fcolorbox", "\\definecolor", "\\pagecolor"], &[], &[], false),
    package("booktabs", &["\\toprule", "\\midrule", "\\bottomrule", "\\cmidrule", "\\addlinespace", "\\specialrule", "\\morecmidrules"], &[], &[], true),
    // Not checkable, because a table can use it through the `S` column alone.
    package("siunitx", &["\\SI", "\\si", "\\num", "\\ang", "\\qty", "\\unit", "\\numlist", "\\numrange", "\\SIlist", "\\SIrange", "\\qtylist", "\\qtyrange", "\\complexnum", "\\complexqty", "\\tablenum", "\\sisetup", "\\DeclareSIUnit"], &[], &[], false),
    package("cleveref", &["\\cref", "\\Cref", "\\crefrange", "\\Crefrange", "\\cpageref", "\\Cpageref", "\\labelcref", "\\namecref", "\\nameCref", "\\lcnamecref", "\\crefname", "\\Crefname", "\\crefformat", "\\Crefformat", "\\crefalias"], &[], &[], true),
    package("listings", &["\\lstinline", "\\lstinputlisting", "\\lstset", "\\lstdefinestyle", "\\lstdefinelanguage", "\\lstnewenvironment"], &["lstlisting"], &[], true),
    package("tikz", &["\\tikz", "\\usetikzlibrary", "\\tikzset", "\\tikzstyle"], &["tikzpicture"], &["xcolor", "graphicx"], true),
    package("multirow", &["\\multirow", "\\multirowsetup"], &[], &[], true),
    package("subcaption", &["\\subcaption", "\\subcaptionbox", "\\subref"], &["subfigure", "subtable"], &["caption"], true),
    package("tabularx", &["\\tabularxcolumn"], &["tabularx"], &[], true),
    package("longtable", &[], &["longtable"], &[], true),
    package("wrapfig", &[], &["wrapfigure", "wraptable"], &[], true),
    package("caption", &["\\captionsetup", "\\captionof"], &[], &[], false),
    package(
        "natbib",
        &["\\citep", "\\citet", "\\citep*", "\\citet*", "\\citealp", "\\citealt", "\\citeauthor", "\\citeyear", "\\citeyearpar"],
        &[],
        &[],
        false,
    ),
    // `\citep` and `\citet` only with the `natbib` option, which a name cannot show.
    package(
        "biblatex",
        &[
            "\\printbibliography", "\\addbibresource", "\\autocite", "\\Autocite", "\\parencite", "\\Parencite", "\\textcite", "\\Textcite",
            "\\footcite", "\\smartcite", "\\fullcite", "\\citeauthor", "\\citeyear", "\\citep", "\\citet",
        ],
        &[],
        &["url"],
        false,
    ),
    package("csquotes", &["\\enquote"], &[], &[], false),
    package("enumitem", &["\\setlist", "\\newlist"], &[], &[], false),
    package("fancyhdr", &["\\fancyhead", "\\fancyfoot", "\\fancyhf"], &[], &[], false),
    package("setspace", &["\\doublespacing", "\\onehalfspacing", "\\singlespacing"], &[], &[], false),
    // Known to load nothing that this table lists.
    package("babel", &[], &[], &[], false),
    package("geometry", &[], &[], &[], false),
    package("inputenc", &[], &[], &[], false),
    package("fontenc", &[], &[], &[], false),
    package("lmodern", &[], &[], &[], false),
    package("microtype", &[], &[], &[], false),
    package("parskip", &[], &[], &[], false),
    package("float", &[], &[], &[], false),
];

/// Classes that provide nothing from the table themselves.
const KNOWN_CLASSES: &[&str] = &["article", "report", "book", "letter", "scrartcl", "scrreprt", "scrbook"];

fn known(name: &str) -> Option<&'static Package> {
    PACKAGES.iter().find(|package| package.name == name)
}

pub(crate) fn package_for_command(command: &str) -> Option<&'static str> {
    PACKAGES.iter().find(|package| package.commands.contains(&command)).map(|package| package.name)
}

pub(crate) fn package_for_environment(environment: &str) -> Option<&'static str> {
    PACKAGES.iter().find(|package| package.environments.contains(&environment)).map(|package| package.name)
}

/// The package whose name is one or two typing mistakes away.
pub(crate) fn closest_package(name: &str) -> Option<&'static str> {
    PACKAGES
        .iter()
        .map(|package| (edit_distance(name, package.name), package.name))
        .filter(|(distance, _)| (1..=2).contains(distance))
        .min_by_key(|(distance, _)| *distance)
        .map(|(_, name)| name)
}

/// What all LaTeX files of a project use and load, taken together, so that a
/// package counts as used when any `\input` part uses it.
#[derive(Clone, Debug, Default)]
pub struct ProjectUsage {
    pub classes: Vec<String>,
    pub packages: Vec<String>,
    pub commands: HashSet<String>,
    pub environments: HashSet<String>,
    pub defined: HashSet<String>,
    /// A local `.sty` or `.cls` can load anything.
    pub has_own_style_files: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PackageFinding {
    /// `item` is the command or environment that needs `package`.
    Missing { package: &'static str, item: String },
    Unused { package: String },
}

/// Every package that the project has for certain, or `None` when it may load
/// more than this table can know.
fn certainly_loaded(usage: &ProjectUsage) -> Option<HashSet<&'static str>> {
    if usage.has_own_style_files || usage.classes.is_empty() || !usage.classes.iter().all(|class| KNOWN_CLASSES.contains(&class.as_str())) {
        return None;
    }
    let mut loaded = HashSet::new();
    let mut waiting = Vec::new();
    for name in &usage.packages {
        waiting.push(known(name)?);
    }
    while let Some(package) = waiting.pop() {
        if loaded.insert(package.name) {
            waiting.extend(package.implies.iter().filter_map(|name| known(name)));
        }
    }
    Some(loaded)
}

/// The packages a project has for certain, for leaving out suggestions that
/// would not work. When unsure, everything counts as available.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Loaded(Option<HashSet<&'static str>>);

pub fn loaded(usage: &ProjectUsage) -> Loaded {
    Loaded(certainly_loaded(usage))
}

impl Loaded {
    /// Adds packages that a file loads now, before the next check sees them.
    /// A name the table does not know may load anything.
    pub fn with<'a>(&self, names: impl IntoIterator<Item = &'a str>) -> Self {
        let Some(loaded) = &self.0 else {
            return Self(None);
        };
        let mut loaded = loaded.clone();
        let mut waiting = Vec::new();
        for name in names {
            match known(name.trim()) {
                Some(package) => waiting.push(package),
                None => return Self(None),
            }
        }
        while let Some(package) = waiting.pop() {
            if loaded.insert(package.name) {
                waiting.extend(package.implies.iter().filter_map(|name| known(name)));
            }
        }
        Self(Some(loaded))
    }

    /// Whether `command` works. Commands outside the table come from LaTeX itself.
    pub fn provides(&self, command: &str) -> bool {
        let Some(loaded) = &self.0 else {
            return true;
        };
        // Asked for every suggestion on every key, so the table is indexed once.
        static PROVIDERS: OnceLock<HashMap<&'static str, Vec<&'static str>>> = OnceLock::new();
        let providers = PROVIDERS.get_or_init(|| {
            let mut providers: HashMap<&'static str, Vec<&'static str>> = HashMap::new();
            for package in PACKAGES {
                for command in package.commands {
                    providers.entry(command).or_default().push(package.name);
                }
            }
            providers
        });
        providers.get(command).is_none_or(|packages| packages.iter().any(|package| loaded.contains(package)))
    }
}

pub fn check(usage: &ProjectUsage) -> Vec<PackageFinding> {
    let mut findings = Vec::new();
    if let Some(loaded) = certainly_loaded(usage) {
        // One finding per package, and the same one for the same text.
        let mut missing: HashMap<&'static str, String> = HashMap::new();
        for package in PACKAGES {
            let items = package
                .commands
                .iter()
                .filter(|command| usage.commands.contains(**command))
                .chain(package.environments.iter().filter(|environment| usage.environments.contains(**environment)));
            for item in items {
                let provided = PACKAGES
                    .iter()
                    .filter(|other| other.commands.contains(item) || other.environments.contains(item))
                    .any(|other| loaded.contains(other.name));
                if provided || usage.defined.contains(*item) {
                    continue;
                }
                let suggested = package_for_command(item).or_else(|| package_for_environment(item)).unwrap_or(package.name);
                let current = missing.entry(suggested).or_insert_with(|| (*item).to_string());
                if **item < **current {
                    *current = (*item).to_string();
                }
            }
        }
        let mut missing: Vec<_> = missing.into_iter().collect();
        missing.sort();
        findings.extend(missing.into_iter().map(|(package, item)| PackageFinding::Missing { package, item }));
    }
    // A local `.sty` or `.cls` can use any package without the project's text showing it.
    if usage.has_own_style_files {
        return findings;
    }
    let mut seen = HashSet::new();
    for name in &usage.packages {
        let Some(package) = known(name).filter(|package| package.checkable) else {
            continue;
        };
        let used = package.commands.iter().any(|command| usage.commands.contains(*command))
            || package.environments.iter().any(|environment| usage.environments.contains(*environment));
        if !used && seen.insert(name) {
            findings.push(PackageFinding::Unused { package: name.clone() });
        }
    }
    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggestions_follow_the_loaded_packages() {
        let plain = loaded(&usage("article", &["amsmath"], &[], &[]));
        assert!(plain.provides("\\cite") && plain.provides("\\section") && plain.provides("\\eqref"));
        assert!(!plain.provides("\\citep") && !plain.provides("\\autocite") && !plain.provides("\\cref"));
        let natbib = loaded(&usage("article", &["natbib"], &[], &[]));
        assert!(natbib.provides("\\citep") && natbib.provides("\\citealp") && !natbib.provides("\\autocite"));
        let biblatex = loaded(&usage("article", &["biblatex"], &[], &[]));
        assert!(biblatex.provides("\\autocite") && biblatex.provides("\\citep"));
        assert!(plain.with(["natbib"]).provides("\\citep"));
        assert!(plain.with(["mathtools"]).provides("\\eqref"));
        // Unsure means everything: an unknown class, package or style file.
        assert!(loaded(&usage("revtex4-2", &[], &[], &[])).provides("\\citep"));
        assert!(plain.with(["somethingelse"]).provides("\\citep"));
        let own = ProjectUsage { has_own_style_files: true, ..usage("article", &[], &[], &[]) };
        assert!(loaded(&own).provides("\\citep"));
        assert!(Loaded::default().provides("\\citep"));
    }

    #[test]
    fn biblatex_counts_for_natbib_commands() {
        let findings = check(&usage("article", &["biblatex"], &["\\citep"], &[]));
        assert!(!findings.iter().any(|finding| matches!(finding, PackageFinding::Missing { .. })), "{findings:?}");
        let findings = check(&usage("article", &[], &["\\citep"], &[]));
        assert!(findings.contains(&PackageFinding::Missing { package: "natbib", item: "\\citep".to_string() }), "{findings:?}");
    }

    fn usage(class: &str, packages: &[&str], commands: &[&str], environments: &[&str]) -> ProjectUsage {
        ProjectUsage {
            classes: vec![class.to_string()],
            packages: packages.iter().map(ToString::to_string).collect(),
            commands: commands.iter().map(ToString::to_string).collect(),
            environments: environments.iter().map(ToString::to_string).collect(),
            ..Default::default()
        }
    }

    fn missing(package: &'static str, item: &str) -> PackageFinding {
        PackageFinding::Missing { package, item: item.to_string() }
    }

    #[test]
    fn finds_what_a_used_command_or_environment_needs() {
        assert_eq!(check(&usage("article", &[], &["\\toprule"], &[])), [missing("booktabs", "\\toprule")]);
        assert_eq!(check(&usage("article", &[], &[], &["align"])), [missing("amsmath", "align")]);
    }

    #[test]
    fn names_one_package_only_once() {
        assert_eq!(check(&usage("article", &[], &["\\toprule", "\\midrule"], &[])), [missing("booktabs", "\\midrule")]);
    }

    #[test]
    fn a_loaded_package_needs_no_hint() {
        assert_eq!(check(&usage("article", &["booktabs"], &["\\toprule"], &[])), []);
    }

    #[test]
    fn silent_about_missing_packages_with_an_unknown_class() {
        assert_eq!(check(&usage("beamer", &[], &["\\toprule"], &[])), []);
        assert_eq!(check(&usage("mythesis", &[], &["\\toprule"], &[])), []);
    }

    #[test]
    fn silent_about_missing_packages_without_a_class() {
        let mut project = usage("article", &[], &["\\toprule"], &[]);
        project.classes.clear();
        assert_eq!(check(&project), []);
    }

    #[test]
    fn silent_about_missing_packages_with_an_unknown_package() {
        assert_eq!(check(&usage("article", &["somethingelse"], &["\\toprule"], &[])), []);
    }

    #[test]
    fn silent_about_missing_packages_with_own_style_files() {
        let mut project = usage("article", &[], &["\\toprule"], &[]);
        project.has_own_style_files = true;
        assert_eq!(check(&project), []);
    }

    #[test]
    fn silent_about_unused_packages_with_own_style_files() {
        let mut project = usage("article", &["booktabs"], &[], &[]);
        project.has_own_style_files = true;
        assert_eq!(check(&project), []);
    }

    #[test]
    fn silent_about_what_the_project_defines() {
        let mut project = usage("article", &[], &["\\toprule"], &["align"]);
        project.defined = ["\\toprule".to_string(), "align".to_string()].into();
        assert_eq!(check(&project), []);
    }

    #[test]
    fn a_package_brings_what_it_loads() {
        assert_eq!(check(&usage("article", &["mathtools"], &["\\text"], &["align"])), []);
        assert_eq!(check(&usage("article", &["tikz"], &["\\tikz", "\\textcolor", "\\includegraphics"], &[])), []);
    }

    #[test]
    fn another_provider_of_a_command_counts() {
        assert_eq!(check(&usage("article", &["amsfonts"], &["\\mathbb"], &[])), []);
        assert_eq!(check(&usage("article", &["url"], &["\\url"], &[])), []);
    }

    #[test]
    fn an_unused_checkable_package_is_found() {
        assert_eq!(check(&usage("article", &["booktabs"], &[], &[])), [PackageFinding::Unused { package: "booktabs".to_string() }]);
    }

    #[test]
    fn unused_works_also_with_an_unknown_class() {
        assert_eq!(check(&usage("beamer", &["booktabs"], &[], &[])), [PackageFinding::Unused { package: "booktabs".to_string() }]);
    }

    #[test]
    fn packages_with_an_effect_are_never_unused() {
        for package in ["babel", "geometry", "hyperref", "inputenc", "fontenc", "microtype", "amsmath", "amssymb", "xcolor", "siunitx", "somethingelse"] {
            assert_eq!(check(&usage("article", &[package], &[], &[])), [], "{package}");
        }
    }

    #[test]
    fn a_used_environment_keeps_a_package() {
        assert_eq!(check(&usage("article", &["tikz"], &[], &["tikzpicture"])), []);
    }

    #[test]
    fn typing_mistakes_in_package_names_are_found() {
        assert_eq!(closest_package("amsmth"), Some("amsmath"));
        assert_eq!(closest_package("completelydifferent"), None);
    }
}
