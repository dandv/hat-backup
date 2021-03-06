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

//! Local state for external blobs and their states.

use sodiumoxide::randombytes::{randombytes};

use std::collections::hashmap::{HashMap};
use serialize::hex::{ToHex};

use process::{Process, MsgHandler};
use sqlite3::database::{Database};

use sqlite3::cursor::{Cursor};
use sqlite3::types::{SQLITE_ROW};
use sqlite3::{open};


pub type BlobIndexProcess = Process<Msg, Reply, BlobIndex>;

#[deriving(Clone, Show)]
pub struct BlobDesc {
  pub name: Vec<u8>,
  pub id: i64,
}

pub enum Msg {
  /// Reserve an internal `BlobDesc` for a new blob.
  Reserve,

  /// Report that this blob is in the process of being committed to persistent storage. If a
  /// blob is in this state when the system starts up, it may or may not exist in the persistent
  /// storage, but **should not** be referenced elsewhere, and is therefore safe to delete.
  InAir(BlobDesc),

  /// Report that this blob has been fully committed to persistent storage. We can now use its
  /// reference internally. Only committed blobs are considered "safe to use".
  CommitDone(BlobDesc),
}

pub enum Reply {
  Reserved(BlobDesc),
  CommitOK,
}

pub struct BlobIndex {
  dbh: Database,
  next_id: i64,
  reserved: HashMap<Vec<u8>, BlobDesc>,
}


impl BlobIndex {

  pub fn new(path: String) -> BlobIndex {
    let mut hi = match open(path.as_slice()) {
      Ok(dbh) => BlobIndex{
        dbh: dbh,
        next_id: -1,
        reserved: HashMap::new(),
      },
      Err(err) => fail!(err.to_string()),
    };
    hi.initialize();
    hi
  }

  #[cfg(test)]
  pub fn new_for_testing() -> BlobIndex {
    BlobIndex::new(":memory:".to_string())
  }

  fn initialize(&mut self) {
    self.exec_or_die("CREATE TABLE IF NOT EXISTS
                      blob_index (id        INTEGER PRIMARY KEY,
                                  name      BLOB,
                                  tag       INT)");
    self.exec_or_die("CREATE UNIQUE INDEX IF NOT EXISTS
                      BlobIndex_UniqueName ON blob_index(name)");
    self.exec_or_die("BEGIN");

    self.refresh_next_id();
  }

  fn new_blob_desc(&mut self) -> BlobDesc {
    BlobDesc{name: randombytes(24),
             id: self.next_id()}
  }

  fn exec_or_die(&mut self, sql: &str) {
    match self.dbh.exec(sql) {
      Ok(true) => (),
      Ok(false) => fail!("exec: {}", self.dbh.get_errmsg()),
      Err(msg) => fail!(format!("exec: {}, {}\nIn sql: '{}'\n",
                                msg.to_string(), self.dbh.get_errmsg(), sql))
    }
  }

  fn prepare_or_die<'a>(&'a self, sql: &str) -> Cursor<'a> {
    match self.dbh.prepare(sql, &None) {
      Ok(s)  => s,
      Err(x) => fail!(format!("sqlite error: {} ({:?})",
                              self.dbh.get_errmsg(), x)),
    }
  }

  fn select1<'a>(&'a mut self, sql: &str) -> Option<Cursor<'a>> {
    let mut cursor = self.prepare_or_die(sql);
    if cursor.step() == SQLITE_ROW {
      Some(cursor)
    } else { None }
  }

  fn refresh_next_id(&mut self) {
    let id = self.select1("SELECT MAX(id) FROM blob_index").unwrap().get_int(0);
    self.next_id = (id as i64) + 1;
  }

  fn next_id(&mut self) -> i64 {
    let id = self.next_id;
    self.next_id += 1;
    id
  }

  fn reserve(&mut self) -> BlobDesc {
    let blob = self.new_blob_desc();
    self.reserved.insert(blob.name.clone(), blob.clone());
    blob
  }

  fn in_air(&mut self, blob: &BlobDesc) {
    assert!(self.reserved.find(&blob.name).is_some(), "blob was not reserved!");
    self.exec_or_die(format!(
      "INSERT INTO blob_index (id, name, tag) VALUES ({}, x'{}', {})",
      blob.id, blob.name.as_slice().to_hex(), 1u).as_slice());
    self.new_transaction();
  }

  fn new_transaction(&mut self) {
    self.exec_or_die("COMMIT; BEGIN");
  }

  fn commit_blob(&mut self, blob: &BlobDesc) {
    assert!(self.reserved.find(&blob.name).is_some(), "blob was not reserved!");
    self.exec_or_die(format!("UPDATE blob_index SET tag=0 WHERE id={}", blob.id).as_slice());
    self.new_transaction();
  }
}

impl Drop for BlobIndex {
  fn drop(&mut self) {
    self.exec_or_die("COMMIT");
  }
}

impl MsgHandler<Msg, Reply> for BlobIndex {
  fn handle(&mut self, msg: Msg, reply: |Reply|) {
    match msg {
      Reserve => {
        return reply(Reserved(self.reserve()));
      },
      InAir(blob) => {
        self.in_air(&blob);
        return reply(CommitOK);
      },
      CommitDone(blob) => {
        self.commit_blob(&blob);
        return reply(CommitOK);
      }
    }
  }
}
