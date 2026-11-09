//! Review by file, for someone who looks at the project later: the author
//! sends the whole project, and the reviewer sends back only what they
//! added, which merges with whatever the author did meanwhile.

use serde::{Deserialize, Serialize};

const MAGIC: &[u8] = b"EDREVIEW1\n";
/// Larger files come only from a broken or hostile sender.
const MAX_UPDATE: usize = 256 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReviewKind {
    /// The project, from its author.
    Request,
    /// Suggestions and comments, from a reviewer.
    Response,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewFile {
    pub kind: ReviewKind,
    pub project: String,
    /// Who wrote the file.
    pub author: String,
    /// Seconds since the Unix epoch.
    pub created: u64,
    /// The state vector of the request, which a response builds on.
    pub base: Vec<u8>,
    #[serde(skip)]
    pub update: Vec<u8>,
}

impl ReviewFile {
    pub fn encode(&self) -> Vec<u8> {
        let meta = serde_json::to_vec(self).unwrap();
        let mut bytes = Vec::with_capacity(MAGIC.len() + 4 + meta.len() + self.update.len());
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&(meta.len() as u32).to_be_bytes());
        bytes.extend_from_slice(&meta);
        bytes.extend_from_slice(&self.update);
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        let invalid = || "This is not a review file".to_string();
        let rest = bytes.strip_prefix(MAGIC).ok_or_else(invalid)?;
        let length = u32::from_be_bytes(rest.get(..4).ok_or_else(invalid)?.try_into().unwrap()) as usize;
        let meta = rest.get(4..4 + length).ok_or_else(invalid)?;
        let update = &rest[4 + length..];
        if update.len() > MAX_UPDATE {
            return Err("The review file is too large".to_string());
        }
        let mut file: Self = serde_json::from_slice(meta).map_err(|_| invalid())?;
        file.update = update.to_vec();
        Ok(file)
    }
}

#[cfg(test)]
mod tests {
    use editor_core::TextChange;

    use super::*;
    use crate::{model::Author, track::track, Project, SuggestionKind};

    fn reviewer() -> Author {
        Author {
            name: "Prof".into(),
            color: 2,
        }
    }

    #[test]
    fn a_review_round_trip_merges_with_what_the_author_did_meanwhile() {
        let mut author = Project::new(1);
        author.add_file("main.tex", "Erster Satz. Zweiter Satz.");
        let request = ReviewFile {
            kind: ReviewKind::Request,
            project: "Thesis".into(),
            author: "Olli".into(),
            created: 0,
            base: author.state_vector(),
            update: author.encode_state(),
        };
        let request = ReviewFile::decode(&request.encode()).unwrap();
        assert_eq!(request.project, "Thesis");

        // The reviewer suggests and comments, offline.
        let mut review = Project::from_state(2, &request.update).unwrap();
        let tracked = track(&[TextChange { removed_text: "Erster".into(), ..TextChange::new(0, 6, "Der erste") }], &[]);
        review.edit_tracked("main.tex", &tracked.ops, &reviewer());
        review.add_comment("main.tex", 13..19, "Passt das?", None, &reviewer());
        let response = ReviewFile {
            kind: ReviewKind::Response,
            project: request.project.clone(),
            author: "Prof".into(),
            created: 0,
            base: request.base.clone(),
            update: review.diff(&request.base).unwrap(),
        };
        let response = ReviewFile::decode(&response.encode()).unwrap();

        // The author kept writing, and takes the review in.
        author.edit("main.tex", &[TextChange::new(26, 0, " Dritter Satz.")]);
        let mut copy = Project::from_state(3, &author.encode_state()).unwrap();
        copy.apply_suggestions_only(&response.update).unwrap();
        author.apply(&response.update).unwrap();
        assert_eq!(author.content("main.tex"), "ErsterDer erste Satz. Zweiter Satz. Dritter Satz.");
        let suggestions = author.suggestions();
        assert_eq!(suggestions.len(), 2);
        assert_eq!(suggestions[0].kind, SuggestionKind::Deleted("Erster".into()));
        assert_eq!(author.comments()[0].text, "Passt das?");
        for suggestion in suggestions {
            author.accept(&suggestion.id);
        }
        assert_eq!(author.content("main.tex"), "Der erste Satz. Zweiter Satz. Dritter Satz.");
    }

    #[test]
    fn a_response_that_edits_directly_is_refused() {
        let mut author = Project::new(1);
        author.add_file("main.tex", "Text");
        let base = author.state_vector();
        let mut review = Project::from_state(2, &author.encode_state()).unwrap();
        review.edit("main.tex", &[TextChange::new(0, 4, "Anders")]);
        let mut copy = Project::from_state(3, &author.encode_state()).unwrap();
        assert!(copy.apply_suggestions_only(&review.diff(&base).unwrap()).is_err());
        assert!(ReviewFile::decode(b"not a review").is_err());
    }
}
