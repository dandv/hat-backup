// Copyright 2014 Google Inc. All rights reserved.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! High level Hat API

use process::{Process};

use blob_index::{BlobIndex, BlobIndexProcess};
use blob_store::{BlobStore, BlobStoreBackend};

use hash_index::{HashIndex, HashIndexProcess};
use hash_tree;

use key_index::{KeyIndex, KeyEntry};

use key_store::{KeyStore, KeyStoreProcess};
use key_store;

use listdir;

use std::io;
use std::io::{Reader, IoResult, UserDir,
              TypeDirectory, TypeSymlink, TypeFile, FileStat};
use std::io::fs::{lstat, File, mkdir_recursive};
use std::sync;

use time;



pub struct Hat<B> {
  repository_root: Path,
  blob_index: BlobIndexProcess,
  hash_index: HashIndexProcess,

  backend: B,
  max_blob_size: uint,
}

fn concat_filename(a: &Path, b: String) -> String {
  let mut result = a.clone();
  result.push(Path::new(b));
  result.as_str().expect("Unable to decode repository_root.").to_string()
}

fn blob_index_name(root: &Path) -> String {
  concat_filename(root, "blob_index.sqlite3".to_string())
}

fn hash_index_name(root: &Path) -> String {
  concat_filename(root, "hash_index.sqlite3".to_string())
}

impl <B: BlobStoreBackend + Clone + Send> Hat<B> {
  pub fn open_repository(repository_root: &Path, backend: B, max_blob_size: uint)
                        -> Option<Hat<B>> {
    repository_root.as_str().map(|_| {
      let blob_index_path = blob_index_name(repository_root);
      let hash_index_path = hash_index_name(repository_root);
      let biP = Process::new(proc() { BlobIndex::new(blob_index_path) });
      let hiP = Process::new(proc() { HashIndex::new(hash_index_path) });
      Hat{repository_root: repository_root.clone(),
                hash_index: hiP,
                blob_index: biP,
                backend: backend.clone(),
                max_blob_size: max_blob_size,
      }
    })
  }

  pub fn open_family(&self, name: String) -> Option<Family<B>> {
    // We setup a standard pipeline of processes:
    // KeyStore -> KeyIndex
    //          -> HashIndex
    //          -> BlobStore -> BlobIndex

    let local_blob_index = self.blob_index.clone();
    let local_backend = self.backend.clone();
    let local_max_blob_size = self.max_blob_size;
    let bsP = Process::new(proc() {
      BlobStore::new(local_blob_index, local_backend, local_max_blob_size) });

    let local_hash_index = self.hash_index.clone();
    let local_hash_index2 = self.hash_index.clone();
    let local_bsP = bsP.clone();

    let key_index_path = concat_filename(&self.repository_root, name.clone());
    let kiP = Process::new(proc() { KeyIndex::new(key_index_path) });

    let ksP = Process::new(proc() { KeyStore::new(kiP, local_hash_index2, bsP) });

    Some(Family{name: name,
                key_store: ksP})
  }
}



struct FileEntry {
  name: Vec<u8>,

  parent_id: Option<Vec<u8>>,

  stat: FileStat,
  full_path: Path,
}

impl FileEntry {
  fn new(full_path: Path,
         parent: Option<Vec<u8>>) -> Result<FileEntry, io::IoError> {
    let filename_opt = full_path.filename();
    if filename_opt.is_some() {
      lstat(&full_path).map(|st| {
        FileEntry{
          name: filename_opt.unwrap().into_vec(),
          parent_id: parent.clone(),
          stat: st,
          full_path: full_path.clone()}
      })
    }
    else { Err(io::IoError{kind: io::OtherIoError,
                           desc: "Could not parse filename.",
                           detail: None }) }
  }

  fn file_iterator(&self) -> IoResult<FileIterator> {
    FileIterator::new(&self.full_path)
  }

  fn is_directory(&self) -> bool { self.stat.kind == TypeDirectory }
  fn is_symlink(&self) -> bool { self.stat.kind == TypeSymlink }
  fn is_file(&self) -> bool { self.stat.kind == TypeFile }
}

impl Clone for FileEntry {
  fn clone(&self) -> FileEntry {
    FileEntry{
      name:self.name.clone(), parent_id:self.parent_id.clone(),
      stat: FileStat{
        size: self.stat.size,
        kind: self.stat.kind,
        perm: self.stat.perm,
        created: self.stat.created,
        modified: self.stat.modified,
        accessed: self.stat.accessed,
        unstable: self.stat.unstable,
      },
      full_path:self.full_path.clone()}
  }
}

impl KeyEntry<FileEntry> for FileEntry {
  fn name(&self) -> Vec<u8> {
    self.name.clone()
  }
  fn id(&self) -> Option<Vec<u8>> {
    Some(format!("d{:u}i{:u}",
                 self.stat.unstable.device,
                 self.stat.unstable.inode).as_bytes().into_vec())
  }
  fn parent_id(&self) -> Option<Vec<u8>> {
    self.parent_id.clone()
  }

  fn size(&self) -> Option<u64> {
    Some(self.stat.size)
  }

  fn created(&self) -> Option<u64> {
    Some(self.stat.created)
  }
  fn modified(&self) -> Option<u64> {
    Some(self.stat.modified)
  }
  fn accessed(&self) -> Option<u64> {
    Some(self.stat.accessed)
  }

  fn permissions(&self) -> Option<u64> {
    None
  }
  fn user_id(&self) -> Option<u64> {
    None
  }
  fn group_id(&self) -> Option<u64> {
    None
  }
  fn with_id(&self, id: Vec<u8>) -> FileEntry {
    assert_eq!(Some(id), self.id());
    self.clone()
  }
}

struct FileIterator {
  file: File
}

impl FileIterator {
  fn new(path: &Path) -> IoResult<FileIterator> {
    match File::open(path) {
      Ok(f) => Ok(FileIterator{file: f}),
      Err(e) => Err(e),
    }
  }
}

impl Iterator<Vec<u8>> for FileIterator {
  fn next(&mut self) -> Option<Vec<u8>> {
    let mut buf = Vec::from_elem(128*1024, 0u8);
    match self.file.read(buf.as_mut_slice()) {
      Err(_) => None,
      Ok(size) => Some(buf.slice_to(size).into_vec()),
    }
  }
}


#[deriving(Clone)]
struct InsertPathHandler<B:'static> {
  count: sync::Arc<sync::Mutex<uint>>,
  last_print: sync::Arc<sync::Mutex<time::Timespec>>,
  my_last_print: time::Timespec,

  key_store: KeyStoreProcess<FileEntry, FileIterator, B>,
}

impl <B> InsertPathHandler<B> {
  pub fn new(key_store: KeyStoreProcess<FileEntry, FileIterator, B>)
             -> InsertPathHandler<B> {
    InsertPathHandler{
      count: sync::Arc::new(sync::Mutex::new(0)),
      last_print: sync::Arc::new(sync::Mutex::new(time::now().to_timespec())),
      my_last_print: time::now().to_timespec(),
      key_store: key_store,
    }
  }
}

impl <B: BlobStoreBackend + Clone + Send> listdir::PathHandler<Option<Vec<u8>>>
  for InsertPathHandler<B> {
  fn handle_path(&mut self, parent: Option<Vec<u8>>, path: Path) -> Option<Option<Vec<u8>>> {
    let count = {
      let mut guarded_count = self.count.lock();
      *guarded_count += 1;
      *guarded_count
    };

    if self.my_last_print.sec <= time::now().to_timespec().sec - 1 {
      let mut guarded_last_print = self.last_print.lock();
      let now = time::now().to_timespec();
      if guarded_last_print.sec <= now.sec - 1 {
        println!("#{}: {}", count, path.display());
        *guarded_last_print = now;
      }
      self.my_last_print = now;
    }

    let fileEntry_opt = FileEntry::new(path.clone(), parent);
    match fileEntry_opt {
      Err(e) => {
        println!("Skipping '{}': {}", path.display(), e.to_string());
      },
      Ok(fileEntry) => {
        if fileEntry.is_symlink() {
          return None;
        }
        let is_directory = fileEntry.is_directory();
        let local_root = path;
        let local_fileEntry = fileEntry.clone();
        let create_file_it = proc() {
          match local_fileEntry.file_iterator() {
            Err(e) => {println!("Skipping '{}': {}", local_root.display(), e.to_string());
                       None},
            Ok(it) => { Some(it) }
          }
        };
        let create_file_it_opt = if is_directory { None }
                                 else { Some(create_file_it) };

        match self.key_store.send_reply(
          key_store::Insert(fileEntry, create_file_it_opt))
        {
          key_store::Id(id) => {
            if is_directory { return Some(Some(id)) }
          },
          _ => fail!("Unexpected reply from key store."),
        }
      }
    }

    return None;
  }
}


fn try_a_few_times_then_fail(f: || -> bool, msg: &str) {
  for i in range(1u, 5) {
    if f() { return }
  }
  fail!(msg.to_string());
}


struct Family<B> {
  name: String,
  key_store: KeyStoreProcess<FileEntry, FileIterator, B>,
}

impl <B: BlobStoreBackend + Clone + Send> Family<B> {

  pub fn snapshot_dir(&self, dir: Path) {
    let mut handler = InsertPathHandler::new(self.key_store.clone());
    listdir::iterate_recursively((Path::new(dir.clone()), None), &mut handler);
  }

  pub fn flush(&self) {
    self.key_store.send_reply(key_store::Flush);
  }

  pub fn checkout_in_dir(&self, output_dir: &mut Path, dir_id: Option<Vec<u8>>) {

    fn put_chunks<B: hash_tree::HashTreeBackend + Clone>(
      fd: &mut File, tree: hash_tree::ReaderResult<B>)
    {
      let mut it = match tree {
        hash_tree::NoData => fail!("Trying to read data where none exist."),
        hash_tree::SingleBlock(chunk) => {
          try_a_few_times_then_fail(|| fd.write(chunk.as_slice()).is_ok(),
                                    "Could not write chunk.");
          return;
        },
        hash_tree::Tree(it) => it,
      };
      // We have a tree
      for chunk in it {
        try_a_few_times_then_fail(|| fd.write(chunk.as_slice()).is_ok(), "Could not write chunk.");
      }
    }

    // create output_dir
    mkdir_recursive(output_dir, UserDir).unwrap();

    let listing = match self.key_store.send_reply(key_store::ListDir(dir_id)) {
      key_store::ListResult(ls) => ls,
      _ => fail!("Unexpected result from key store."),
    };

    for (id, name, _, _, _, hash, _, data_res) in listing.move_iter() {

      output_dir.push(name);

      if hash.len() == 0 {
        // This is a directory, recurse!
        self.checkout_in_dir(output_dir, Some(id));
      } else {
        // This is a file, write it
        let mut fd = File::create(output_dir).unwrap();
        put_chunks(&mut fd, data_res);
        try_a_few_times_then_fail(|| fd.flush().is_ok(), "Could not flush file.")
      }

      output_dir.pop();
    }

  }
}
