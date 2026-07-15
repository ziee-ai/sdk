// File permissions

use ziee_identity::PermissionCheck;

pub struct FilesRead;
impl PermissionCheck for FilesRead {
    const NAME: &'static str = "FilesRead";
    const PERMISSION: &'static str = "files::read";
    const DESCRIPTION: &'static str = "View file metadata and list files";
    const MODULE: &'static str = "file";
}

pub struct FilesUpload;
impl PermissionCheck for FilesUpload {
    const NAME: &'static str = "FilesUpload";
    const PERMISSION: &'static str = "files::upload";
    const DESCRIPTION: &'static str = "Upload new files";
    const MODULE: &'static str = "file";
}

pub struct FilesDownload;
impl PermissionCheck for FilesDownload {
    const NAME: &'static str = "FilesDownload";
    const PERMISSION: &'static str = "files::download";
    const DESCRIPTION: &'static str = "Download file content";
    const MODULE: &'static str = "file";
}

pub struct FilesDelete;
impl PermissionCheck for FilesDelete {
    const NAME: &'static str = "FilesDelete";
    const PERMISSION: &'static str = "files::delete";
    const DESCRIPTION: &'static str = "Delete files";
    const MODULE: &'static str = "file";
}

pub struct FilesPreview;
impl PermissionCheck for FilesPreview {
    const NAME: &'static str = "FilesPreview";
    const PERMISSION: &'static str = "files::preview";
    const DESCRIPTION: &'static str = "View file thumbnails and previews";
    const MODULE: &'static str = "file";
}

pub struct FilesGenerateToken;
impl PermissionCheck for FilesGenerateToken {
    const NAME: &'static str = "FilesGenerateToken";
    const PERMISSION: &'static str = "files::generate_token";
    const DESCRIPTION: &'static str = "Generate download tokens";
    const MODULE: &'static str = "file";
}

#[cfg(test)]
mod tests {
    use super::{
        FilesDelete, FilesDownload, FilesGenerateToken, FilesPreview, FilesRead, FilesUpload,
    };
    use ziee_identity::{PermissionCheck, PermissionList};

    /// The `files::*` permission keys feed the OpenAPI 403 example (which the UI
    /// `Permissions` enum is scraped from) via `RequirePermissions`/
    /// `with_permission`. Pin the exact string + name + module + derived
    /// resource/action for every key so a rename can't silently drop one from
    /// the generated enum. Mirrors `ziee-hardware`'s
    /// `hardware_permission_keys_are_stable`.
    #[test]
    fn files_permission_keys_are_stable() {
        assert_eq!(FilesRead::PERMISSION, "files::read");
        assert_eq!(FilesRead::NAME, "FilesRead");
        assert_eq!(FilesRead::MODULE, "file");
        assert_eq!(FilesRead::resource(), "files");
        assert_eq!(FilesRead::action(), "read");

        assert_eq!(FilesUpload::PERMISSION, "files::upload");
        assert_eq!(FilesUpload::NAME, "FilesUpload");
        assert_eq!(FilesUpload::MODULE, "file");
        assert_eq!(FilesUpload::action(), "upload");

        assert_eq!(FilesDownload::PERMISSION, "files::download");
        assert_eq!(FilesDownload::NAME, "FilesDownload");
        assert_eq!(FilesDownload::MODULE, "file");
        assert_eq!(FilesDownload::action(), "download");

        assert_eq!(FilesDelete::PERMISSION, "files::delete");
        assert_eq!(FilesDelete::NAME, "FilesDelete");
        assert_eq!(FilesDelete::MODULE, "file");
        assert_eq!(FilesDelete::action(), "delete");

        assert_eq!(FilesPreview::PERMISSION, "files::preview");
        assert_eq!(FilesPreview::NAME, "FilesPreview");
        assert_eq!(FilesPreview::MODULE, "file");
        assert_eq!(FilesPreview::action(), "preview");

        assert_eq!(FilesGenerateToken::PERMISSION, "files::generate_token");
        assert_eq!(FilesGenerateToken::NAME, "FilesGenerateToken");
        assert_eq!(FilesGenerateToken::MODULE, "file");
        // `action()` is the LAST "::" segment — "generate_token".
        assert_eq!(FilesGenerateToken::resource(), "files");
        assert_eq!(FilesGenerateToken::action(), "generate_token");
    }

    /// A tuple of the file keys surfaces the ordered permission set the extractor
    /// AND-checks (the `PermissionList` collect the OpenAPI generator walks).
    /// `PermissionList` is impl'd up to a 4-tuple, so this pins four; the six
    /// individual keys above cover the rest.
    #[test]
    fn files_permission_tuple_collects_in_order() {
        type Core = (FilesRead, FilesUpload, FilesDownload, FilesDelete);
        assert_eq!(
            <Core as PermissionList>::permissions(),
            vec!["files::read", "files::upload", "files::download", "files::delete"]
        );
        assert_eq!(
            <Core as PermissionList>::names(),
            vec!["FilesRead", "FilesUpload", "FilesDownload", "FilesDelete"]
        );
    }
}
