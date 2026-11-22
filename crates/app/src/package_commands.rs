//! The commands and environments that LaTeX packages define, read from their
//! `.sty` files, so that suggestions know any package the project loads, not
//! only the ones in our own list.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    process::Command,
    time::UNIX_EPOCH,
};

use serde::{Deserialize, Serialize};

/// Files read per check, so that a package that loads half of TeX Live does
/// not stall the next one.
const MAX_FILES_PER_CHECK: usize = 20;
/// How long a file that is not installed is not asked for again.
const MISSING_RECHECK_SECONDS: u64 = 24 * 60 * 60;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageCommand {
    pub name: String,
    /// In the slot format of `editor_core::snippet`.
    pub template: String,
    pub package: String,
}

/// What one `.sty` defines.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct Definitions {
    /// Names with their templates.
    pub commands: Vec<(String, String)>,
    pub environments: Vec<String>,
    /// The packages it loads itself.
    pub requires: Vec<String>,
    /// Files it reads as part of itself, such as `biblatex.def`.
    pub inputs: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
struct CacheEntry {
    path: PathBuf,
    modified: u64,
    definitions: Definitions,
}

type Cache = HashMap<String, CacheEntry>;

/// The commands and environments of `packages` and of what they load, one
/// level deep. `local` holds the `.sty` files of the project, which win over
/// the installed ones. Blocks, so call it in the background.
pub fn collect(packages: &[String], local: &[PathBuf]) -> (Vec<PackageCommand>, Vec<(String, String)>) {
    let cache_path = cache_path();
    let mut cache: Cache = cache_path.as_deref().map(load_cache).unwrap_or_default();
    let changed = refresh(&mut cache, packages, local, find_installed);
    if changed && let Some(path) = &cache_path {
        save_cache(path, &cache).ok();
    }
    let mut commands = Vec::new();
    let mut environments = Vec::new();
    let mut seen = HashSet::new();
    let required: Vec<&String> = packages.iter().flat_map(|name| cache.get(&sty(name))).flat_map(|entry| &entry.definitions.requires).collect();
    for name in packages.iter().chain(required) {
        let Some(entry) = cache.get(&sty(name)).filter(|_| seen.insert(name.clone())) else {
            continue;
        };
        // A package and the files it reads count as one.
        let parts = std::iter::once(entry).chain(entry.definitions.inputs.iter().filter_map(|file| cache.get(file)));
        for part in parts {
            commands.extend(part.definitions.commands.iter().map(|(command, template)| PackageCommand {
                name: command.clone(),
                template: template.clone(),
                package: name.clone(),
            }));
            environments.extend(part.definitions.environments.iter().map(|environment| (environment.clone(), name.clone())));
        }
    }
    (commands, environments)
}

/// Reads what is missing or changed. `find` locates installed `.sty` files.
/// Returns whether the cache changed.
fn refresh(cache: &mut Cache, packages: &[String], local: &[PathBuf], find: impl Fn(&[&str]) -> Vec<PathBuf>) -> bool {
    let mut changed = false;
    let mut budget = MAX_FILES_PER_CHECK;
    let mut level: Vec<String> = packages.iter().map(|name| sty(name)).collect();
    for _ in 0..2 {
        let stale: Vec<&str> = level.iter().filter(|file| cache.get(*file).is_none_or(is_stale)).map(String::as_str).collect();
        let mut paths: Vec<PathBuf> = local.iter().filter(|path| stale.iter().any(|file| is_file(path, file))).cloned().collect();
        let installed: Vec<&str> = stale.iter().copied().filter(|file| !paths.iter().any(|path| is_file(path, file))).collect();
        if !installed.is_empty() {
            let found = find(&installed);
            // Remembered as missing, so that a typo does not ask on every build.
            for file in installed.iter().filter(|file| !found.iter().any(|path| is_file(path, file))) {
                cache.insert(file.to_string(), CacheEntry { path: PathBuf::new(), modified: now(), definitions: Definitions::default() });
                changed = true;
            }
            paths.extend(found);
        }
        for path in paths.into_iter().take(budget) {
            let (Some(file), Some(modified)) = (path.file_name().map(|name| name.to_string_lossy().into_owned()), modified(&path)) else {
                continue;
            };
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            budget -= 1;
            cache.insert(file, CacheEntry { path, modified, definitions: parse(&text) });
            changed = true;
        }
        // What the packages load, and the files they read as part of themselves.
        level = level
            .iter()
            .filter_map(|file| cache.get(file))
            .flat_map(|entry| entry.definitions.requires.iter().map(|name| sty(name)).chain(entry.definitions.inputs.iter().cloned()))
            .collect();
    }
    changed
}

fn is_stale(entry: &CacheEntry) -> bool {
    if entry.path.as_os_str().is_empty() {
        return now().saturating_sub(entry.modified) > MISSING_RECHECK_SECONDS;
    }
    modified(&entry.path) != Some(entry.modified)
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |time| time.as_secs())
}

fn sty(name: &str) -> String {
    format!("{name}.sty")
}

fn is_file(path: &Path, file: &str) -> bool {
    path.file_name().is_some_and(|name| name.to_string_lossy() == file)
}

fn modified(path: &Path) -> Option<u64> {
    let time = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(time.duration_since(UNIX_EPOCH).ok()?.as_secs())
}

/// One `kpsewhich` call for all files. Without one, as with Tectonic alone,
/// there is nothing to find.
fn find_installed(files: &[&str]) -> Vec<PathBuf> {
    let Some(kpsewhich) = build::find_tool("kpsewhich") else {
        return Vec::new();
    };
    let Ok(output) = Command::new(kpsewhich).args(files).output() else {
        return Vec::new();
    };
    String::from_utf8_lossy(&output.stdout).lines().map(str::trim).filter(|line| !line.is_empty()).map(PathBuf::from).collect()
}

fn cache_path() -> Option<PathBuf> {
    Some(crate::storage::cache_dir()?.join("packages.json"))
}

fn load_cache(path: &Path) -> Cache {
    std::fs::read_to_string(path).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

fn save_cache(path: &Path, cache: &Cache) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string(cache).map_err(std::io::Error::other)?)
}

const COMMAND_DEFINERS: &[&str] = &["newcommand", "providecommand", "DeclareRobustCommand"];
const DOCUMENT_DEFINERS: &[&str] = &["NewDocumentCommand", "DeclareDocumentCommand", "ProvideDocumentCommand"];
const ENVIRONMENT_DEFINERS: &[&str] = &["newenvironment", "NewDocumentEnvironment", "DeclareDocumentEnvironment"];
/// biblatex: each takes the key.
const CITE_DEFINERS: &[&str] = &["DeclareCiteCommand", "DeclareMultiCiteCommand"];

/// The public definitions of a `.sty`. Names with `@`, `_` or `:` are
/// internal, and `\def` is left out, since packages use it mostly for those.
pub fn parse(text: &str) -> Definitions {
    let text = without_comments(text);
    let mut definitions = Definitions::default();
    let mut names = HashSet::new();
    let mut rest = text.as_str();
    while let Some(at) = rest.find('\\') {
        rest = &rest[at + 1..];
        let word_length = rest.bytes().take_while(u8::is_ascii_alphabetic).count();
        let word = &rest[..word_length];
        let after = rest[word_length..].trim_start().trim_start_matches('*').trim_start();
        if COMMAND_DEFINERS.contains(&word) || DOCUMENT_DEFINERS.contains(&word) || CITE_DEFINERS.contains(&word) {
            let Some((name, after)) = defined_name(after) else {
                continue;
            };
            let slots = if DOCUMENT_DEFINERS.contains(&word) {
                group(after).map_or(0, |(spec, _)| mandatory(spec))
            } else if CITE_DEFINERS.contains(&word) {
                1
            } else {
                classic_slots(after)
            };
            if is_public(name) && names.insert(name.to_string()) {
                definitions.commands.push((format!("\\{name}"), format!("\\{name}{}", "{${}}".repeat(slots))));
            }
        } else if ENVIRONMENT_DEFINERS.contains(&word) {
            if let Some((name, _)) = group(after).filter(|(name, _)| is_public(name.trim_end_matches('*')))
                && !definitions.environments.iter().any(|known| known == name)
            {
                definitions.environments.push(name.to_string());
            }
        } else if word == "RequirePackage" {
            let after = if after.starts_with('[') { after.split_once(']').map_or("", |(_, rest)| rest.trim_start()) } else { after };
            if let Some((list, _)) = group(after) {
                definitions.requires.extend(list.split(',').map(str::trim).filter(|name| !name.is_empty()).map(str::to_string));
            }
        }
    }
    // Files named in braces, as `\input{tikz.code.tex}` or `\blx@inputonce{biblatex.def}` read them.
    for (index, _) in text.match_indices('{') {
        let Some((name, _)) = group(&text[index..]) else {
            continue;
        };
        let part = name.strip_suffix(".def").or_else(|| name.strip_suffix(".code.tex"));
        if part.is_some_and(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'))
            && !definitions.inputs.iter().any(|known| known == name)
        {
            definitions.inputs.push(name.to_string());
        }
    }
    definitions
}

fn without_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        let mut escaped = false;
        let end = line
            .char_indices()
            .find(|(_, ch)| {
                let comment = *ch == '%' && !escaped;
                escaped = *ch == '\\' && !escaped;
                comment
            })
            .map_or(line.len(), |(index, _)| index);
        out.push_str(&line[..end]);
        out.push('\n');
    }
    out
}

/// The name after a definer, as `{\name}` or `\name`, and the text after it.
fn defined_name(text: &str) -> Option<(&str, &str)> {
    let (inner, braced) = match text.strip_prefix('{') {
        Some(inner) => (inner.trim_start(), true),
        None => (text, false),
    };
    let inner = inner.strip_prefix('\\')?;
    // Inside a `.sty`, `@` is a letter.
    let length = inner.bytes().take_while(|byte| byte.is_ascii_alphabetic() || matches!(byte, b'@' | b'_' | b':')).count();
    if length == 0 {
        return None;
    }
    let mut after = &inner[length..];
    if braced {
        after = after.trim_start().strip_prefix('}')?;
    }
    Some((&inner[..length], after.trim_start()))
}

fn is_public(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(|byte| byte.is_ascii_alphabetic())
}

/// `[n]` arguments, of which a `[default]` makes the first optional.
fn classic_slots(text: &str) -> usize {
    let Some(rest) = text.strip_prefix('[') else {
        return 0;
    };
    let Some((count, rest)) = rest.split_once(']') else {
        return 0;
    };
    let count: usize = count.trim().parse().unwrap_or(0);
    let optional = rest.trim_start().starts_with('[');
    count.saturating_sub(usize::from(optional)).min(9)
}

/// A braced group at the start of `text`, and the text after it.
fn group(text: &str) -> Option<(&str, &str)> {
    let inner = text.strip_prefix('{')?;
    let mut depth = 0usize;
    for (index, byte) in inner.bytes().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' if depth == 0 => return Some((inner[..index].trim(), &inner[index + 1..])),
            b'}' => depth -= 1,
            _ => {}
        }
    }
    None
}

/// The mandatory arguments of an xparse spec such as `s o +m O{x} m`.
fn mandatory(spec: &str) -> usize {
    let mut count = 0;
    let mut chars = spec.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            'm' | 'v' => count += 1,
            // A default or a delimiter pair, which is no argument of its own.
            '{' => {
                let mut depth = 1;
                for inner in chars.by_ref() {
                    match inner {
                        '{' => depth += 1,
                        '}' if depth == 1 => break,
                        '}' => depth -= 1,
                        _ => {}
                    }
                }
            }
            't' => {
                chars.next();
            }
            'r' | 'R' | 'd' | 'D' => {
                chars.next();
                chars.next();
            }
            _ => {}
        }
    }
    count.min(9)
}

#[cfg(test)]
mod tests {
    use super::*;

    const STY: &str = r"
\NeedsTeXFormat{LaTeX2e}
\RequirePackage[strict]{etoolbox, kvoptions}
\RequirePackage{xparse}
\blx@inputonce{biblatex.def}{generic definitions}{}{}{}{}
\input{tikz.code.tex}
\DeclareCiteCommand{\textcite}[\mkbibparens]{}{}{}{}
\DeclareCiteCommand*{\citeauthor}{}{}{}{}
% \newcommand\commented{x}
\DeclareRobustCommand\citep{\begingroup\NAT@swafalse}
\newcommand*{\citealp}[1]{#1}
\newcommand{\gls}[2][]{#2}
\providecommand \glsdesc [3] {#1}
\newcommand\NAT@internal{}
\newcommand{\my_expl:n}{}
\def\citeplain{}
\NewDocumentCommand \qty { O { } m m } {}
\NewDocumentCommand\SI{ s t+ D<>{x} +m }{}
\newenvironment{thebib}[1]{}{}
\NewDocumentEnvironment{glossarytable*}{m}{}{}
\newcommand{\citep}{again}
50\% of \newcommand\after{}
";

    #[test]
    fn public_definitions_and_their_arguments() {
        let definitions = parse(STY);
        let commands: Vec<(&str, &str)> = definitions.commands.iter().map(|(name, template)| (name.as_str(), template.as_str())).collect();
        assert_eq!(
            commands,
            [
                ("\\textcite", "\\textcite{${}}"),
                ("\\citeauthor", "\\citeauthor{${}}"),
                ("\\citep", "\\citep"),
                ("\\citealp", "\\citealp{${}}"),
                ("\\gls", "\\gls{${}}"),
                ("\\glsdesc", "\\glsdesc{${}}{${}}{${}}"),
                ("\\qty", "\\qty{${}}{${}}"),
                ("\\SI", "\\SI{${}}"),
                ("\\after", "\\after"),
            ]
        );
        assert_eq!(definitions.environments, ["thebib", "glossarytable*"]);
        assert_eq!(definitions.requires, ["etoolbox", "kvoptions", "xparse"]);
        assert_eq!(definitions.inputs, ["biblatex.def", "tikz.code.tex"]);
    }

    #[test]
    fn the_cache_reads_a_file_once_until_it_changes() {
        let directory = tempfile::tempdir().unwrap();
        let sty = directory.path().join("niche.sty");
        std::fs::write(&sty, "\\newcommand\\niche{}\\RequirePackage{helper}").unwrap();
        let helper = directory.path().join("helper.sty");
        std::fs::write(&helper, "\\newcommand\\helped[1]{}").unwrap();
        let calls = std::cell::Cell::new(0);
        let find = |files: &[&str]| {
            calls.set(calls.get() + 1);
            files.iter().filter(|file| **file == "helper.sty").map(|_| helper.clone()).collect()
        };
        let mut cache = Cache::new();
        let local = vec![sty.clone()];
        let packages = vec!["niche".to_string()];
        assert!(refresh(&mut cache, &packages, &local, find));
        assert_eq!(cache["helper.sty"].definitions.commands, [("\\helped".to_string(), "\\helped{${}}".to_string())]);
        assert_eq!(calls.get(), 1);
        // Nothing changed, so nothing is read and nobody is asked.
        assert!(!refresh(&mut cache, &packages, &local, find));
        assert_eq!(calls.get(), 1);
        // A changed file counts by its time.
        cache.get_mut("niche.sty").unwrap().modified -= 1;
        assert!(refresh(&mut cache, &packages, &local, find));
        // A package that is not installed is asked for once, then a day later.
        let typo = vec!["nopackage".to_string()];
        let asked = calls.get();
        assert!(refresh(&mut cache, &typo, &[], find));
        assert!(!refresh(&mut cache, &typo, &[], find));
        assert_eq!(calls.get(), asked + 1);
        cache.get_mut("nopackage.sty").unwrap().modified -= MISSING_RECHECK_SECONDS + 1;
        refresh(&mut cache, &typo, &[], find);
        assert_eq!(calls.get(), asked + 2);
    }

    #[test]
    #[ignore = "needs TeX Live, prints counts and a timing, run with --ignored --nocapture"]
    fn installed_packages_are_read() {
        let names: Vec<String> = ["natbib", "siunitx", "glossaries", "tikz", "biblatex"].map(String::from).to_vec();
        let mut cache = Cache::new();
        // The budget spreads a cold start over a few checks.
        for check in 1..=10 {
            let started = std::time::Instant::now();
            let changed = refresh(&mut cache, &names, &[], find_installed);
            println!("check {check}: {:?}, {} files known", started.elapsed(), cache.len());
            if !changed {
                break;
            }
        }
        for name in &names {
            let entry = &cache[&sty(name)];
            let parts = std::iter::once(entry).chain(entry.definitions.inputs.iter().filter_map(|file| cache.get(file)));
            let commands: usize = parts.map(|part| part.definitions.commands.len()).sum();
            println!("{name}: {commands} commands, inputs {:?}", entry.definitions.inputs);
        }
    }
}
