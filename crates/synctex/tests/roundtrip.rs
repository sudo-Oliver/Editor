//! Checks navigation against real pdfLaTeX output. Skipped when pdflatex is missing.

use std::process::Command;

use synctex::SyncTex;

fn pdflatex() -> Option<&'static str> {
    ["pdflatex", "/Library/TeX/texbin/pdflatex"]
        .into_iter()
        .find(|program| Command::new(program).arg("--version").output().is_ok())
}

#[test]
fn navigates_multi_page_document_with_input_file() {
    let Some(pdflatex) = pdflatex() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::create_dir(root.join("sub")).unwrap();
    std::fs::write(
        root.join("main.tex"),
        "\\documentclass{article}\n\\begin{document}\nFirst page text.\n\\newpage\nSecond page text.\n\\newpage\n\\input{sub/part}\n\\end{document}\n",
    )
    .unwrap();
    std::fs::write(root.join("sub/part.tex"), "Included text.\n\nMore included text.\n").unwrap();

    let status = Command::new(pdflatex)
        .args(["-interaction=nonstopmode", "-synctex=1", "main.tex"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(status.status.success());

    let synctex = SyncTex::open(&root.join("main.synctex.gz")).unwrap();
    let main = root.join("main.tex");
    let part = root.join("sub/part.tex");

    assert_eq!(synctex.forward(&main, 3).unwrap().page, 0);
    assert_eq!(synctex.forward(&main, 5).unwrap().page, 1);
    let included = synctex.forward(&part, 1).unwrap();
    assert_eq!(included.page, 2);
    let later = synctex.forward(&part, 3).unwrap();
    assert!(later.y > included.y, "{later:?} is not below {included:?}");

    for (file, line) in [(&main, 3), (&main, 5), (&part, 1), (&part, 3)] {
        let location = synctex.forward(file, line).unwrap();
        let back = synctex
            .backward(location.page, location.x + 1.0, location.y + location.height / 2.0)
            .unwrap();
        assert_eq!((&back.file, back.line), (file, line));
    }
}
