use walkdir::WalkDir;
use std::{
    collections::VecDeque,
    io,
    path::PathBuf,
    fs,
};

pub struct DirectoryWalker {
    walker: walkdir::IntoIter,
    directories: VecDeque<PathBuf>,
    restricted_files: Vec<PathBuf>,
    restricted_dirs: Vec<PathBuf>,
}

impl DirectoryWalker {
    pub fn new(root: &str) -> Self {
        DirectoryWalker {
            walker: WalkDir::new(root).into_iter(),
            directories: VecDeque::new(),
            restricted_files: Vec::new(),
            restricted_dirs: Vec::new(),
        }
    }

    pub fn next_chunk(&mut self, chunk_size: usize) -> Vec<PathBuf> {
        let mut chunk = Vec::new();
        let mut count = 0;
        while count < chunk_size {
            if let Some(Ok(entry)) = self.walker.next() {
                let path = entry.path().to_path_buf();
                
                match fs::metadata(&path) {
                    Ok(metadata) => {
                        if metadata.is_dir() {
                            self.directories.push_front(path);
                        } else if metadata.is_file() {
                            chunk.push(path);
                            count += 1;
                        }
                    }
                    Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {
                        if path.is_dir() {
                            self.restricted_dirs.push(path);
                        } else {
                            self.restricted_files.push(path);
                        }
                    }
                    Err(_) => {}
                }
            } else {
                break;
            }
        }
        chunk
    }

    pub fn next_dir_chunk(&mut self, chunk_size: usize) -> Vec<PathBuf> {
        self.directories.drain(..chunk_size.min(self.directories.len())).collect()
    }

    pub fn get_directories(&self) -> Vec<PathBuf> {
        self.directories.iter().cloned().collect()
    }
    pub fn get_restricted_files(&self) -> Vec<PathBuf> {
        self.restricted_files.clone()
    }
    pub fn get_restricted_dirs(&self) -> Vec<PathBuf> {
        self.restricted_dirs.clone()
    }



}

