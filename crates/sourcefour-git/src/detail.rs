//! Full metadata for one selected commit (§6.10).

use smallvec::SmallVec;
use sourcefour_model::{CommitDetail, GitTime, Oid, RepoFailure, RepoLocation, Signature};

use crate::{files::missing, history::convert_oid, history::open_failure};

/// Reads one commit's complete metadata: subject, body, both identities, and
/// parent IDs (§6.10). This is the cheap read that fills the details header.
///
/// # Errors
///
/// Returns a typed failure when the repository or the commit cannot be read.
pub fn commit_detail(location: &RepoLocation, oid: Oid) -> Result<CommitDetail, RepoFailure> {
    let repository =
        gix::open(&location.git_dir).map_err(|error| open_failure(&location.git_dir, &error))?;
    let commit = repository
        .find_commit(gix::ObjectId::from_bytes_or_panic(oid.as_bytes()))
        .map_err(|error| missing(oid, &error))?;
    let decoded = commit.decode().map_err(|error| missing(oid, &error))?;
    let message = decoded.message();
    Ok(CommitDetail {
        oid,
        subject: message.summary().to_string(),
        body: message.body.map_or_else(String::new, |body| {
            String::from_utf8_lossy(body).trim_end().to_owned()
        }),
        author: signature(decoded.author()),
        committer: signature(decoded.committer()),
        parents: decoded
            .parents()
            .filter_map(|parent| convert_oid(parent.as_ref()))
            .collect::<SmallVec<[Oid; 2]>>(),
    })
}

/// Owns a borrowed signature, tolerating an unparseable identity or timestamp.
fn signature(parsed: Result<gix::actor::SignatureRef<'_>, gix::objs::decode::Error>) -> Signature {
    let Ok(reference) = parsed else {
        return Signature {
            name: String::new(),
            email: String::new(),
            time: GitTime {
                seconds_since_epoch: 0,
                offset_minutes: 0,
            },
        };
    };
    let time = reference.time().map_or(
        GitTime {
            seconds_since_epoch: 0,
            offset_minutes: 0,
        },
        |time| GitTime {
            seconds_since_epoch: time.seconds,
            offset_minutes: time.offset / 60,
        },
    );
    Signature {
        name: reference.name.to_string(),
        email: reference.email.to_string(),
        time,
    }
}

#[cfg(test)]
mod tests {
    use sourcefour_model::Oid;
    use sourcefour_test_support::TempRepo;

    use super::commit_detail;
    use crate::discover;

    fn head(repository: &TempRepo) -> Result<Oid, Box<dyn std::error::Error>> {
        Ok(Oid::from_hex(&repository.git(&["rev-parse", "HEAD"]))?)
    }

    #[test]
    fn a_detail_carries_subject_body_and_identities() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        repository.commit("subject line\n\nThe body explains why.\nOver two lines.");

        let detail = commit_detail(&discover(repository.path())?, head(&repository)?)?;

        assert_eq!(detail.subject, "subject line");
        assert_eq!(detail.body, "The body explains why.\nOver two lines.");
        assert_eq!(detail.author.name, "Sourcefour Fixture");
        assert_eq!(detail.author.email, "fixture@sourcefour.invalid");
        assert_eq!(detail.committer.name, "Sourcefour Fixture");
        assert_eq!(detail.parents.len(), 1);
        Ok(())
    }

    #[test]
    fn a_subject_only_commit_has_an_empty_body() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        repository.commit("just a subject");

        let detail = commit_detail(&discover(repository.path())?, head(&repository)?)?;

        assert_eq!(detail.subject, "just a subject");
        assert!(detail.body.is_empty());
        Ok(())
    }

    #[test]
    fn a_merge_detail_lists_both_parents() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        repository.git(&["checkout", "-q", "-b", "side"]);
        repository.commit("side work");
        repository.git(&["checkout", "-q", "main"]);
        repository.commit("main work");
        repository.git(&["merge", "-q", "--no-ff", "side", "-m", "merge side"]);

        let detail = commit_detail(&discover(repository.path())?, head(&repository)?)?;

        assert_eq!(detail.parents.len(), 2);
        Ok(())
    }

    #[test]
    fn a_missing_object_is_a_typed_failure() {
        let repository = TempRepo::init();
        let location = discover(repository.path()).expect("fixture repository discovers");

        let failure =
            commit_detail(&location, Oid::sha1([0xAB; 20])).expect_err("object does not exist");

        assert_eq!(
            failure.kind,
            sourcefour_model::RepoFailureKind::MissingObject
        );
    }
}
