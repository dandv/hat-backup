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

//! Helpers for reading directory structures from the local filesystem.

use std::sync;
use std::os::{last_os_error};
use std::io::{TypeDirectory};
use std::io::fs::{lstat};

use std::c_str::CString;
use libc::funcs::posix88::dirent;
use libc::types::common::posix88::{DIR,dirent_t};
use libc::types::os::arch::c95::c_char;
use libc::{c_int};


pub struct DirIterator {
  fd: *mut DIR,
}

impl DirIterator {
  pub fn new(path: Path) -> Result<DirIterator, String> {
    let fd = path.with_c_str(|c_str| unsafe { dirent::opendir(c_str) });

    if fd as int > 0 { Ok(DirIterator{fd:fd}) }
    else { Err(last_os_error()) }
  }

  fn read(&mut self) -> String {
    extern {
      fn rust_dirent_t_size() -> c_int;
      fn rust_list_dir_val(ptr: *mut dirent_t) -> *const c_char;
    }

    let mut entry_ptr = 0 as *mut dirent_t;

    let size = unsafe { rust_dirent_t_size() };
    let mut buf = Vec::<u8>::with_capacity(size as uint);
    let buf_ptr = buf.as_mut_ptr() as *mut dirent_t;

    let retval = unsafe { dirent::readdir_r(self.fd, buf_ptr, &mut entry_ptr) };

    if retval == 0 && !entry_ptr.is_null() {
      let cstr = unsafe { CString::new(rust_list_dir_val(entry_ptr), false) };
      cstr.as_str().expect("Path not UTF8.").into_string()
    } else { "".to_string() }

  }

}

impl Drop for DirIterator {
  fn drop(&mut self) {
    unsafe {
      dirent::closedir(self.fd)
    };
  }
}

impl Iterator<String> for DirIterator {
  fn next(&mut self) -> Option<String> {
    let name = self.read();
    if name.len() == 0 { None }
    else { Some(name) }
  }
}


pub trait PathHandler<D> {
  fn handle_path(&mut self, D, Path) -> Option<D>;
}


pub fn iterate_recursively<P: Send + Clone, W: PathHandler<P> + Send + Clone>
  (root: (Path, P), worker: &mut W)
{
  let threads = 5;
  let (push_ch, work_ch) = sync_channel(threads);
  let mut pool = sync::TaskPool::new(threads, || proc(_){()});

  // Insert the first task into the queue:
  push_ch.send(Some(root));
  let mut running_workers = 0i;

  // Master thread:
  loop {
    match work_ch.recv() {
      None => {
        // A worker has reached completion and has decremented the counter.
        // We are done when no more workers are active (i.e. all tasks are done):
        running_workers -= 1;
        if running_workers == 0 {
          break
        }
      },
      Some((root, payload)) => {
        let t_worker = worker.clone();
        let t_push_ch = push_ch.clone();

        // Execute the task in a pool thread:
        running_workers += 1;
        pool.execute(proc(&()) {
          let mut root = root;
          let mut t_worker = t_worker;
          let res = DirIterator::new(root.clone());
          if res.is_ok() {
            let mut it = res.unwrap();
            for file in it {
              if file != ".".into_string() && file != "..".into_string() {
                let rel_path = Path::new(file);
                root.push(rel_path);
                let dir_opt = t_worker.handle_path(payload.clone(), root.clone());
                if dir_opt.is_some() {
                  t_push_ch.send(Some((root.clone(), dir_opt.unwrap())));
                }
                root.pop();
              }
            }
          }

          // Count this pool thread as idle:
          t_push_ch.send(None)
        });
      }
    }
  }
}

struct PrintPathHandler;

impl Clone for PrintPathHandler {
  fn clone(&self) -> PrintPathHandler { PrintPathHandler }
}

impl PathHandler<()> for PrintPathHandler {
  fn handle_path(&mut self, _: (), path: Path) -> Option<()> {
    let filename_opt = path.filename_str();
    println!("{}", path.display());
    match filename_opt {
      Some(".") | Some("..") | None => None,
      Some(_) =>
      match lstat(&path) {
        Ok(ref st) if st.kind == TypeDirectory => Some(()),
        _ => None,
      }
    }
  }
}
