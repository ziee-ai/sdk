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
