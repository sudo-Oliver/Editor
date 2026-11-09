//! A guest without a window, for trying a session by hand:
//! `cargo run -p collab --example guest -- <invite code>`. It joins, types a
//! line at the start of main.tex, shows its cursor there, comments on it,
//! suggests an addition and a deletion, and leaves after a while.

use std::time::{Duration, Instant};

use collab::{Author, Event, Identity, Project, Session, random_client_id, track};
use editor_core::TextChange;

fn main() {
    let code = std::env::args().nth(1).expect("pass the invite code");
    let identity = Identity { name: "Grace".into(), color: 0xd9480f };
    let author = Author { name: identity.name.clone(), color: identity.color };
    let session = Session::join(&code, identity, || {}).expect("join");
    let mut project = Project::new(random_client_id());
    let start = Instant::now();
    let mut typed = false;
    while start.elapsed() < Duration::from_secs(40) {
        for event in session.poll() {
            match event {
                Event::Update(update) => {
                    project.apply(&update).expect("apply");
                    for (path, changes) in project.take_remote() {
                        println!("remote {path}: {} change(s)", changes.len());
                    }
                }
                other => println!("{other:?}"),
            }
        }
        if !typed && !project.content("main.tex").is_empty() {
            typed = true;
            session.send_update(project.edit("main.tex", &[TextChange::new(0, 0, "% Hello from Grace\n")]));
            session.presence("main.tex", 2, 7);
            session.send_update(project.add_comment("main.tex", 2..7, "Is this greeting needed?", None, &author));
            let suggestion = track(&[TextChange::new(18, 0, " and friends")], &[]);
            session.send_update(project.edit_tracked("main.tex", &suggestion.ops, &author));
            // Suggests deleting a word, which stays struck through.
            if let Some(at) = project.content("main.tex").find("Language") {
                let deletion = TextChange { removed_text: "Language".into(), ..TextChange::new(at, 8, "") };
                let suggestion = track(&[deletion], &[]);
                session.send_update(project.edit_tracked("main.tex", &suggestion.ops, &author));
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
