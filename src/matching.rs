use crate::model::{Candidate, Query};
use std::collections::BTreeSet;

/// Strong evidence is a complete title and artist match, not keyword relevance.
/// Version suffixes, punctuation and artist credits are intentionally retained.
pub fn reliable(query: &Query, candidate: &Candidate) -> bool {
    let Some(title) = query
        .title
        .as_deref()
        .map(normalize)
        .filter(|s| !s.is_empty())
    else {
        return false;
    };
    if title != normalize(&candidate.title) {
        return false;
    }
    let Some(artists) = artist_set(&query.artists) else {
        return false;
    };
    if artist_set(&candidate.artists).as_ref() != Some(&artists) {
        return false;
    }
    if let Some(album) = &query.album {
        let expected = normalize(album);
        if expected.is_empty()
            || candidate.album.as_deref().map(normalize).as_deref() != Some(expected.as_str())
        {
            return false;
        }
    }
    if let Some(duration) = query.duration {
        if !duration.is_finite() || duration <= 0.0 {
            return false;
        }
        let Some(actual) = candidate.duration else {
            return false;
        };
        if !actual.is_finite() || actual <= 0.0 || (duration - actual).abs() > 2.0 {
            return false;
        }
    }
    true
}

fn normalize(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn artist_set(artists: &[String]) -> Option<BTreeSet<String>> {
    let normalized: BTreeSet<_> = artists.iter().map(|artist| normalize(artist)).collect();
    (!normalized.is_empty() && !normalized.contains("")).then_some(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Provider, SongRef};

    fn track() -> Candidate {
        Candidate {
            song: SongRef::new(Provider::Netease, "1").unwrap(),
            title: "A Song (Live)".into(),
            artists: vec!["Alice".into(), "Bob".into()],
            album: Some("Live Album".into()),
            duration: Some(180.0),
        }
    }

    #[test]
    fn matches_complete_metadata_and_artist_order() {
        let candidate = track();
        let mut query = candidate.query();
        query.title = Some("  a   song (live) ".into());
        query.artists = vec!["BOB".into(), "alice".into()];
        assert!(reliable(&query, &candidate));
        query.duration = Some(182.0);
        assert!(reliable(&query, &candidate));
        query.duration = Some(182.001);
        assert!(!reliable(&query, &candidate));
    }

    #[test]
    fn does_not_strip_versions_or_accept_partial_artists() {
        let candidate = track();
        let mut query = candidate.query();
        query.title = Some("A Song".into());
        assert!(!reliable(&query, &candidate));
        query = candidate.query();
        query.artists.pop();
        assert!(!reliable(&query, &candidate));
        query = candidate.query();
        query.artists.push("Carol".into());
        assert!(!reliable(&query, &candidate));
    }

    #[test]
    fn keywords_and_empty_evidence_never_auto_select() {
        let candidate = track();
        let mut query = Query {
            keywords: "Alice A Song (Live)".into(),
            ..Query::default()
        };
        assert!(!reliable(&query, &candidate));
        query.title = Some(candidate.title.clone());
        assert!(!reliable(&query, &candidate));
        query.artists = vec![" ".into()];
        assert!(!reliable(&query, &candidate));
        query = candidate.query();
        query.title = Some(" ".into());
        assert!(!reliable(&query, &candidate));
    }

    #[test]
    fn requires_trusted_album_and_duration_to_be_available() {
        let mut candidate = track();
        let query = candidate.query();
        candidate.album = None;
        assert!(!reliable(&query, &candidate));
        candidate = track();
        candidate.album = Some("Studio Album".into());
        assert!(!reliable(&query, &candidate));
        candidate = track();
        candidate.duration = None;
        assert!(!reliable(&query, &candidate));
        for duration in [f64::NAN, f64::INFINITY, 0.0, -1.0] {
            candidate.duration = Some(duration);
            assert!(!reliable(&query, &candidate));
            let mut bad_query = track().query();
            bad_query.duration = Some(duration);
            assert!(!reliable(&bad_query, &track()));
        }
    }
}
