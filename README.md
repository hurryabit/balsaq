# balsaq

Proc-macro library that eliminates rusqlite boilerplate. Annotate your Rust
structs; balsaq generates `CREATE TABLE`, `SELECT`, and `INSERT` SQL as
compile-time string constants, and implements the row-mapping code.

```toml
[dependencies]
balsaq   = "0.1"
rusqlite = "0.39"
```

## Quick start

```rust
use balsaq::Model as _;
use rusqlite::Connection;

#[balsaq::table("users")]
pub struct User {
    #[primary_key]
    pub id:    i64,
    pub name:  String,
    pub score: f64,
}

fn main() -> rusqlite::Result<()> {
    let conn = Connection::open_in_memory()?;
    conn.execute_batch(User::CREATE_TABLE)?;

    balsaq::insert(&conn, User { id: 1, name: "Alice".into(), score: 9.5 })?;

    let user = User::get(&conn, &1i64)?;
    println!("{} — {}", user.name, user.score);
    Ok(())
}
```

`#[balsaq::table("users")]` generates:

```rust
// CREATE TABLE IF NOT EXISTS users (
//     id    INTEGER NOT NULL,
//     name  TEXT    NOT NULL,
//     score REAL    NOT NULL,
//     PRIMARY KEY (id)
// );
User::CREATE_TABLE: &'static str

// SELECT id, name, score FROM users
User::SELECT: &'static str

// INSERT OR IGNORE INTO users (id, name, score) VALUES (:id, :name, :score)
User::INSERT: &'static str
```

## Column types

Any type that implements `Column` can be used as a field. balsaq ships impls
for the common Rust primitives:

| Rust type    | SQL type                           |
|--------------|------------------------------------|
| `i32`, `i64` | `INTEGER`                          |
| `f64`        | `REAL`                             |
| `String`     | `TEXT`                             |
| `Vec<u8>`    | `BLOB`                             |
| `[u8; N]`    | `BLOB`                             |
| `bool`       | `INTEGER CHECK ({col} IN (0, 1))`  |
| `Option<T>`  | same as `T`, but nullable          |

### Custom column types

Use `#[derive(balsaq::Column)]` on a **newtype wrapper** or a **C-like enum**:

```rust
// Newtype: delegates SQL_TYPE to the inner type.
#[derive(balsaq::Column)]
pub struct UserId(i64);

// Enum: stored as INTEGER with a CHECK constraint on the discriminant values.
#[derive(balsaq::Column)]
#[repr(i64)]
pub enum Status { Active = 1, Inactive = 2 }
```

For types that don't fit either pattern, implement `Column` manually alongside
the existing `ToSql`/`FromSql` impls.

## Column groups

A **column group** is a struct whose fields each become a SQL column, all
sharing a common prefix. Derive `ColumnGroup` with `#[balsaq::group]`:

```rust
#[balsaq::group]
pub struct AuthorInfo {
    pub name: String,
    pub bio:  Option<String>,
}

#[balsaq::group]
pub struct Sig {
    pub data: Vec<u8>,
    pub hash: Vec<u8>,
}
```

Embed a group in a table with `#[group]`:

```rust
#[balsaq::table("posts")]
pub struct Post {
    #[primary_key]
    pub id:     i64,
    pub title:  String,
    #[group]
    pub author: AuthorInfo,  // author_name TEXT NOT NULL, author_bio TEXT
    #[group]
    pub sig:    Option<Sig>, // sig_data BLOB, sig_hash BLOB
}
```

When a group is optional (`Option<Sig>`), all of its columns become nullable.
Reading back a row returns `None` when every column of the group is `NULL`.

### Via adapter

When the in-memory type can't implement `Column` directly, map it through a
raw group with `via = RawType`. balsaq uses `From` to write and `TryFrom` to
read back:

```rust
// In-memory type — not directly storable in SQLite.
pub enum Value {
    Number(i64),
    Text(String),
}

// Raw group: two nullable columns, exactly one populated per row.
#[balsaq::group]
pub struct ValueRaw {
    pub number: Option<i64>,
    pub text:   Option<String>,
}
impl From<Value> for ValueRaw { /* … */ }
impl TryFrom<ValueRaw> for Value { /* … */ }

#[balsaq::table("entries")]
pub struct Entry {
    #[primary_key]
    pub id:    i64,
    #[group(via = ValueRaw)]
    pub value: Value,
}
```

## Indexes

Declare indexes with `#[index(...)]` on the table struct:

```rust
#[balsaq::table("posts")]
#[index(title)]
#[index(id, title, unique = true)]
pub struct Post { /* … */ }
```

Index DDL is appended to `CREATE_TABLE`.

## Schema constant

Bundle multiple tables into a single `SCHEMA` constant with `#[balsaq::schema]`:

```rust
#[balsaq::schema]
pub mod db {
    #[balsaq::table("users")]  pub struct User  { /* … */ }
    #[balsaq::table("posts")]  pub struct Post  { /* … */ }
}

conn.execute_batch(db::SCHEMA)?;   // creates all tables in one call
```

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.
