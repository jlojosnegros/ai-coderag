use std::fs;
use std::path::Path;

pub fn read_file_to_string(path: &Path) -> std::io::Result<String> {
    fs::read_to_string(path)
}

pub fn write_string_to_file(path: &Path, content: &str) -> std::io::Result<()> {
    fs::write(path, content)
}

pub fn list_files_in_directory(dir: &Path) -> std::io::Result<Vec<std::path::PathBuf>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            files.push(entry.path());
        }
    }
    Ok(files)
}

pub fn file_exists(path: &Path) -> bool {
    path.exists() && path.is_file()
}