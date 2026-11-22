//! Times the builds of a large real project, such as a thesis. Never runs by
//! itself: point it at a copy of the project, since it writes `.build` there
//! and edits one file.
//!
//!   EDITOR_BENCH_PROJECT=/path/to/copy EDITOR_BENCH_EDIT=chapters/04-results.tex \
//!     cargo test --release -p build --test large_project -- --ignored --nocapture

use std::{path::PathBuf, time::Instant};

use build::{BuildEvent, BuildManager, BuildOutcome, BuildRequest, Config, Engine, find_main};

fn run(manager: &BuildManager, request: &BuildRequest) -> BuildOutcome {
    let mut events = manager.start(request.clone());
    loop {
        match events.blocking_recv() {
            Some(BuildEvent::Finished(outcome)) => return outcome,
            Some(BuildEvent::Cancelled) | None => panic!("the build stopped"),
            Some(_) => {}
        }
    }
}

#[test]
#[ignore]
fn builds_of_a_large_project() {
    let Some(root) = std::env::var_os("EDITOR_BENCH_PROJECT").map(PathBuf::from) else {
        eprintln!("EDITOR_BENCH_PROJECT is not set");
        return;
    };
    let config = Config::load(&root).unwrap();
    let main = find_main(&root, &config, Some(&root.join("main.tex"))).expect("a main file");
    let engine = std::env::var("EDITOR_BENCH_ENGINE").ok().map_or(Engine::PdfLatex, |name| match name.as_str() {
        "tectonic" => Engine::Tectonic,
        "lualatex" => Engine::LuaLatex,
        _ => Engine::PdfLatex,
    });
    let manager = BuildManager::new();
    let request = BuildRequest { root: root.clone(), main, engine, clean: true };
    let report = |label: &str, outcome: &BuildOutcome| {
        let errors = outcome.diagnostics.iter().filter(|diagnostic| diagnostic.severity == build::Severity::Error).count();
        println!("{label:<28} {:>7.2} s  success={} pdf={} errors={errors}", outcome.duration.as_secs_f64(), outcome.success, outcome.pdf.is_some());
        for diagnostic in outcome.diagnostics.iter().filter(|diagnostic| diagnostic.severity == build::Severity::Error) {
            println!("    {:?}:{:?} {}", diagnostic.file, diagnostic.line, diagnostic.message);
        }
    };
    let started = Instant::now();
    let clean = run(&manager, &request);
    report("clean", &clean);
    let warm = BuildRequest { clean: false, ..request.clone() };
    for index in 1..=3 {
        report(&format!("unchanged {index}"), &run(&manager, &warm));
    }
    if let Some(edit) = std::env::var_os("EDITOR_BENCH_EDIT").map(|edit| root.join(edit)) {
        let original = std::fs::read_to_string(&edit).unwrap();
        for index in 1..=3 {
            std::fs::write(&edit, format!("{original}\nEdit {index}.\n")).unwrap();
            report(&format!("after an edit {index}"), &run(&manager, &warm));
        }
        std::fs::write(&edit, &original).unwrap();
    }
    println!("total {:.1} s", started.elapsed().as_secs_f64());
}
