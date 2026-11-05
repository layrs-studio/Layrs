//! Bounded ordinary-file input for an expected provider ticket. The returned
//! bytes are untrusted DATA, never a native/SQL capability or an immutable file.
use crate::secure_fs::PinnedDirectory;
use std::{
    io::{self, Read},
    path::Path,
};

const MAX_BYTES: u64 = 128 * 1024;

/// Read an ordinary ticket through retained, directory-relative no-follow
/// handles. Decode and authenticate its expected references separately.
pub fn read_provider_ticket_document(path: &Path) -> io::Result<Vec<u8>> {
    let absolute = std::path::absolute(path)?;
    let name = absolute
        .file_name()
        .ok_or_else(|| invalid("ticket has no file name"))?;
    let text = name
        .to_str()
        .ok_or_else(|| invalid("ticket name is not Unicode"))?;
    if text.contains(':') || text.ends_with(['.', ' ']) {
        return Err(invalid("ticket name is not an ordinary file name"));
    }
    let parent = PinnedDirectory::open(
        absolute
            .parent()
            .ok_or_else(|| invalid("ticket has no parent"))?,
    )?;
    parent.verify_path_identity()?;
    let pinned = parent.open_readonly_file(name)?;
    let length = pinned.file().metadata()?.len();
    if length == 0 || length > MAX_BYTES {
        return Err(invalid("provider ticket size exceeds its bound"));
    }
    let mut bytes = Vec::with_capacity(length as usize);
    pinned.file().take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 != length || pinned.file().metadata()?.len() != length {
        return Err(invalid("provider ticket size changed while reading"));
    }
    parent.verify_path_identity()?;
    if !pinned.same_file(&parent.open_readonly_file(name)?) {
        return Err(invalid("provider ticket identity changed while reading"));
    }
    parent.verify_path_identity()?;
    Ok(bytes)
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projection::provider::{database_io::tests::support, process_profile::tests::child};
    use std::fs;

    #[test]
    #[ignore = "owned audited Windows host; retained ordinary ticket inputs and ancestor junction refusal"]
    fn ordinary_ticket_is_bounded_and_does_not_follow_ancestor_junction() {
        child::run(
            concat!(
                module_path!(),
                "::ordinary_ticket_is_bounded_and_does_not_follow_ancestor_junction"
            ),
            120,
            || {
                child::declare();
                let root = support::owned_root("provider-ticket");
                let ticket = root.join("ticket.json");
                support::create(&ticket, b"{}");
                let mut permissions = fs::metadata(&ticket).unwrap().permissions();
                permissions.set_readonly(true);
                fs::set_permissions(&ticket, permissions).unwrap();
                assert_eq!(read_provider_ticket_document(&ticket).unwrap(), b"{}");
                let oversized = root.join("oversized.json");
                support::create(&oversized, &vec![b' '; MAX_BYTES as usize + 1]);
                assert!(read_provider_ticket_document(&oversized).is_err());
                assert!(read_provider_ticket_document(&root).is_err());
                let target = root.join("target");
                fs::create_dir(&target).unwrap();
                support::create(&target.join("ticket.json"), b"{}");
                let link = root.join("junction");
                support::junction(&root, &link, &target);
                assert!(read_provider_ticket_document(&link.join("ticket.json")).is_err());
                assert_eq!(fs::read(&ticket).unwrap(), b"{}");
                assert_eq!(fs::read(target.join("ticket.json")).unwrap(), b"{}");
            },
        );
    }
}
