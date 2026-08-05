//! One document's bytes, from whichever of a diff's three sides shows it, and
//! the images that document references.

use sourcefour_model::{DiffParent, Oid, RepoLocation, RepoPath};

use crate::diff::{blob_at, image_format, index_blob, worktree_bytes};

/// Where a previewed document's bytes live.
#[derive(Clone, Debug)]
pub enum DocSource {
    /// A blob in one commit's tree.
    Commit {
        /// The commit whose tree is read.
        oid: Oid,
        /// The document's repository-relative path.
        path: RepoPath,
    },
    /// The staged blob.
    Index {
        /// The document's repository-relative path.
        path: RepoPath,
    },
    /// The file on disk under the active worktree.
    Worktree {
        /// The document's repository-relative path.
        path: RepoPath,
    },
}

/// The commit a diff's old side reads from, `None` when the old side is
/// the empty tree — a root commit has no old document to render.
pub fn parent_commit_oid(location: &RepoLocation, oid: Oid, parent: DiffParent) -> Option<Oid> {
    match parent {
        DiffParent::Parent(parent) => Some(parent),
        DiffParent::EmptyTree => None,
        DiffParent::FirstParent => {
            let repository = gix::open(&location.git_dir).ok()?;
            let commit = repository
                .find_commit(gix::ObjectId::from_bytes_or_panic(oid.as_bytes()))
                .ok()?;
            let parent = commit.parent_ids().next()?;
            crate::history::convert_oid(parent.as_ref())
        }
    }
}

/// What resolving one image reference produced.
#[derive(Clone, Debug)]
pub enum ImageResolution {
    /// Bytes plus the renderable format name (png/jpeg/gif/webp/bmp/tiff).
    Found {
        /// The image's bytes.
        bytes: Vec<u8>,
        /// The format name the viewer renders with.
        format: String,
    },
    /// An http(s) URL: never fetched, the preview shows a placeholder.
    Remote,
    /// Not found anywhere it may be looked for, or not a renderable format.
    Missing,
}

impl DocSource {
    /// The same source, addressing another path beside the document.
    fn sibling(&self, path: RepoPath) -> Self {
        match self {
            Self::Commit { oid, .. } => Self::Commit { oid: *oid, path },
            Self::Index { .. } => Self::Index { path },
            Self::Worktree { .. } => Self::Worktree { path },
        }
    }
}

/// The document's bytes from its source, `None` when absent there.
///
/// A source that cannot be read at all — an unopenable repository, an unknown
/// commit, an unreadable index — is absence too: a preview renders what it can
/// find and says nothing about the rest.
pub fn document_bytes(location: &RepoLocation, source: &DocSource) -> Option<Vec<u8>> {
    match source {
        DocSource::Worktree { path } => worktree_bytes(location, &path.0),
        DocSource::Commit { oid, path } => {
            let repository = gix::open(&location.git_dir).ok()?;
            let commit = repository
                .find_commit(gix::ObjectId::from_bytes_or_panic(oid.as_bytes()))
                .ok()?;
            blob_at(Some(&commit.tree().ok()?), &path.0)
        }
        DocSource::Index { path } => {
            let repository = gix::open(&location.git_dir).ok()?;
            index_blob(&repository, &path.0).ok().flatten()
        }
    }
}

/// Resolves one image reference from a document at `doc_path`, read from
/// `source`. Relative references join against the document's parent
/// directory with `.`/`..` normalization and never escape the repository
/// root. Lookup order: the document's own source, then the worktree as a
/// fallback — an image deleted from the tree still renders when previewing
/// the commit that had it.
pub fn resolve_doc_image(
    location: &RepoLocation,
    source: &DocSource,
    doc_path: &RepoPath,
    reference: &str,
) -> ImageResolution {
    if reference.starts_with("http://") || reference.starts_with("https://") {
        return ImageResolution::Remote;
    }
    let Some(path) = referenced_path(doc_path, reference) else {
        return ImageResolution::Missing;
    };
    // Nothing to gain by reading a file the viewer cannot decode.
    let Some(format) = image_format(&path) else {
        return ImageResolution::Missing;
    };
    let path = RepoPath(path);
    document_bytes(location, &source.sibling(path.clone()))
        .or_else(|| worktree_bytes(location, &path.0))
        .map_or(ImageResolution::Missing, |bytes| ImageResolution::Found {
            bytes,
            format,
        })
}

/// The repository-relative path a reference from `doc_path` names, `None` when
/// it climbs out of the repository root.
///
/// Segments are normalized over bytes: a document path is raw bytes that need
/// not be UTF-8, and Git separates path components with `/` on every platform.
fn referenced_path(doc_path: &RepoPath, reference: &str) -> Option<Vec<u8>> {
    // Badges and anchors carry suffixes that are not part of the path.
    let reference = &reference[..reference.find(['?', '#']).unwrap_or(reference.len())];
    // A leading slash means repository-root-relative, not filesystem-absolute.
    let base: &[u8] = if reference.starts_with('/') {
        &[]
    } else {
        parent_of(&doc_path.0)
    };

    let mut segments: Vec<&[u8]> = Vec::new();
    for segment in base
        .split(|byte| *byte == b'/')
        .chain(reference.as_bytes().split(|byte| *byte == b'/'))
    {
        match segment {
            b"" | b"." => {}
            b".." => {
                segments.pop()?;
            }
            named => segments.push(named),
        }
    }
    Some(segments.join(&b'/'))
}

/// Everything before a path's last `/`, empty for a root-level document.
fn parent_of(path: &[u8]) -> &[u8] {
    path.iter()
        .rposition(|byte| *byte == b'/')
        .map_or(&[], |slash| &path[..slash])
}

#[cfg(test)]
mod tests {
    use sourcefour_model::{Oid, RepoPath};
    use sourcefour_test_support::TempRepo;

    use super::{DocSource, ImageResolution, document_bytes, resolve_doc_image};
    use crate::discover;

    fn head(repository: &TempRepo) -> Result<Oid, Box<dyn std::error::Error>> {
        Ok(Oid::from_hex(&repository.git(&["rev-parse", "HEAD"]))?)
    }

    fn path(value: &str) -> RepoPath {
        RepoPath(value.as_bytes().to_vec())
    }

    fn write(repository: &TempRepo, relative: &str, bytes: &[u8]) -> std::io::Result<()> {
        let target = repository.path().join(relative);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(target, bytes)
    }

    #[test]
    fn the_old_side_resolves_to_the_parent_commit() -> Result<(), Box<dyn std::error::Error>> {
        use sourcefour_model::DiffParent;

        let repository = TempRepo::init();
        repository.commit("second");
        let second = head(&repository)?;
        let parent = Oid::from_hex(&repository.git(&["rev-parse", "HEAD^"]))?;
        let root = Oid::from_hex(&repository.git(&["rev-list", "--max-parents=0", "HEAD"]))?;
        let location = discover(repository.path())?;

        assert_eq!(
            super::parent_commit_oid(&location, second, DiffParent::FirstParent),
            Some(parent)
        );
        assert_eq!(
            super::parent_commit_oid(&location, root, DiffParent::FirstParent),
            None,
            "a root commit has no old side"
        );
        assert_eq!(
            super::parent_commit_oid(&location, second, DiffParent::Parent(parent)),
            Some(parent),
            "an explicit parent is taken at its word"
        );
        assert_eq!(
            super::parent_commit_oid(&location, second, DiffParent::EmptyTree),
            None
        );
        Ok(())
    }

    #[test]
    fn the_three_sources_read_three_payloads() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        write(&repository, "note.md", b"committed\n")?;
        repository.git(&["add", "."]);
        repository.commit("add note");
        write(&repository, "note.md", b"staged\n")?;
        repository.git(&["add", "."]);
        write(&repository, "note.md", b"on disk\n")?;

        let location = discover(repository.path())?;
        let oid = head(&repository)?;

        assert_eq!(
            document_bytes(
                &location,
                &DocSource::Commit {
                    oid,
                    path: path("note.md"),
                },
            )
            .as_deref(),
            Some(b"committed\n".as_slice())
        );
        assert_eq!(
            document_bytes(
                &location,
                &DocSource::Index {
                    path: path("note.md")
                }
            )
            .as_deref(),
            Some(b"staged\n".as_slice())
        );
        assert_eq!(
            document_bytes(
                &location,
                &DocSource::Worktree {
                    path: path("note.md"),
                },
            )
            .as_deref(),
            Some(b"on disk\n".as_slice())
        );
        Ok(())
    }

    #[test]
    fn an_image_deleted_later_still_renders_from_its_commit()
    -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        write(
            &repository,
            "docs/readme.md",
            b"![logo](../assets/logo.png)\n",
        )?;
        write(&repository, "assets/logo.png", b"\x89PNG-old")?;
        repository.git(&["add", "."]);
        repository.commit("add docs");
        let old = head(&repository)?;
        repository.git(&["rm", "assets/logo.png"]);
        repository.commit("drop the logo");

        let resolution = resolve_doc_image(
            &discover(repository.path())?,
            &DocSource::Commit {
                oid: old,
                path: path("docs/readme.md"),
            },
            &path("docs/readme.md"),
            "../assets/logo.png",
        );

        let ImageResolution::Found { bytes, format } = resolution else {
            panic!("the commit's own tree still holds the image");
        };
        assert_eq!(bytes, b"\x89PNG-old");
        assert_eq!(format, "png");
        Ok(())
    }

    #[test]
    fn a_reference_to_nothing_is_missing() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        write(&repository, "readme.md", b"![gone](ghost.png)\n")?;
        repository.git(&["add", "."]);
        repository.commit("add readme");

        let resolution = resolve_doc_image(
            &discover(repository.path())?,
            &DocSource::Commit {
                oid: head(&repository)?,
                path: path("readme.md"),
            },
            &path("readme.md"),
            "ghost.png",
        );

        assert!(matches!(resolution, ImageResolution::Missing));
        Ok(())
    }

    #[test]
    fn parent_segments_resolve_but_never_escape_the_root() -> Result<(), Box<dyn std::error::Error>>
    {
        let repository = TempRepo::init();
        write(&repository, "docs/readme.md", b"docs\n")?;
        write(&repository, "assets/x.png", b"\x89PNG-x")?;
        let location = discover(repository.path())?;
        let source = DocSource::Worktree {
            path: path("docs/readme.md"),
        };

        let inside = resolve_doc_image(
            &location,
            &source,
            &path("docs/readme.md"),
            "../assets/x.png",
        );
        let ImageResolution::Found { bytes, .. } = inside else {
            panic!("a parent segment resolves against the document's directory");
        };
        assert_eq!(bytes, b"\x89PNG-x");

        assert!(
            matches!(
                resolve_doc_image(
                    &location,
                    &source,
                    &path("docs/readme.md"),
                    "../../assets/x.png",
                ),
                ImageResolution::Missing
            ),
            "a reference may not climb out of the repository"
        );
        Ok(())
    }

    #[test]
    fn an_http_reference_is_remote_and_never_fetched() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        write(&repository, "readme.md", b"badge\n")?;
        let location = discover(repository.path())?;
        let source = DocSource::Worktree {
            path: path("readme.md"),
        };

        for reference in [
            "https://img.shields.io/x.svg",
            "http://example.invalid/x.png",
        ] {
            assert!(matches!(
                resolve_doc_image(&location, &source, &path("readme.md"), reference),
                ImageResolution::Remote
            ));
        }
        Ok(())
    }

    #[test]
    fn a_query_or_fragment_suffix_still_resolves() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        write(&repository, "readme.md", b"readme\n")?;
        write(&repository, "logo.png", b"\x89PNG")?;
        let location = discover(repository.path())?;
        let source = DocSource::Worktree {
            path: path("readme.md"),
        };

        for reference in ["logo.png?raw=true", "logo.png#top"] {
            assert!(
                matches!(
                    resolve_doc_image(&location, &source, &path("readme.md"), reference),
                    ImageResolution::Found { .. }
                ),
                "{reference} names logo.png"
            );
        }
        Ok(())
    }

    #[test]
    fn the_worktree_backs_up_a_commit_that_lacks_the_image()
    -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        write(&repository, "readme.md", b"![new](new.png)\n")?;
        repository.git(&["add", "."]);
        repository.commit("add readme");
        // Written after the commit, so only the filesystem has it.
        write(&repository, "new.png", b"\x89PNG-new")?;

        let resolution = resolve_doc_image(
            &discover(repository.path())?,
            &DocSource::Commit {
                oid: head(&repository)?,
                path: path("readme.md"),
            },
            &path("readme.md"),
            "new.png",
        );

        let ImageResolution::Found { bytes, .. } = resolution else {
            panic!("the worktree is the fallback when the tree lacks the image");
        };
        assert_eq!(bytes, b"\x89PNG-new");
        Ok(())
    }

    #[test]
    fn a_format_the_viewer_cannot_render_is_missing() -> Result<(), Box<dyn std::error::Error>> {
        let repository = TempRepo::init();
        write(&repository, "readme.md", b"![vector](logo.svg)\n")?;
        write(&repository, "logo.svg", b"<svg/>")?;
        let location = discover(repository.path())?;

        let resolution = resolve_doc_image(
            &location,
            &DocSource::Worktree {
                path: path("readme.md"),
            },
            &path("readme.md"),
            "logo.svg",
        );

        assert!(
            matches!(resolution, ImageResolution::Missing),
            "the viewer decodes no svg, so a present file is still missing"
        );
        Ok(())
    }
}
