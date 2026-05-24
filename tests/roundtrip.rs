use balsaq::{Column, Model, group, insert, schema, table};
use rusqlite::{
    Connection,
    types::{FromSql, FromSqlError, FromSqlResult, ToSql, ToSqlOutput, ValueRef},
};
use std::convert::TryFrom;

// ── Newtype column derive ─────────────────────────────────────────────────────

#[derive(Column, Clone, Copy, PartialEq, Debug)]
pub struct Karma(i64);

#[derive(Column, Clone, PartialEq, Debug)]
pub struct Label(String);

// ── Enum column derive ────────────────────────────────────────────────────────

#[repr(i64)]
#[derive(Column, Clone, Copy, PartialEq, Debug)]
pub enum Status {
    Active = 1,
    Inactive = 2,
    Deleted = 3,
}

// ── Stub ID type ──────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlobId([u8; 4]);

impl BlobId {
    fn new(b: u8) -> Self {
        Self([b; 4])
    }
}

impl ToSql for BlobId {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::Borrowed(ValueRef::Blob(&self.0)))
    }
}

impl FromSql for BlobId {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let bytes: Vec<u8> = Vec::column_result(value)?;
        let arr: [u8; 4] = bytes
            .try_into()
            .map_err(|_| rusqlite::types::FromSqlError::InvalidType)?;
        Ok(Self(arr))
    }
}

impl balsaq::Column for BlobId {
    const SQL_TYPE: &'static str = "BLOB";
    const NULLABLE: bool = false;
}

// ── Schema module ─────────────────────────────────────────────────────────────

#[schema]
pub mod catalog {
    use balsaq::table;

    #[table("alpha")]
    pub struct Alpha {
        #[primary_key]
        pub id: i64,
    }

    #[table("beta")]
    pub struct Beta {
        #[primary_key]
        pub id: i64,
        pub label: String,
    }
}

// ── Plain two-column table ────────────────────────────────────────────────────

#[table("widgets")]
#[index(name)]
pub struct Widget {
    #[primary_key]
    pub id: BlobId,
    pub name: String,
}

// ── Column group types ────────────────────────────────────────────────────────

#[group]
pub struct AuthorInfo {
    pub name: String,
    pub karma: i64,
}

/// Optional column group — used as `Option<Sig>` in a table struct.
#[group]
pub struct Sig {
    pub data: Vec<u8>,
    pub hash: Vec<u8>,
}

/// Group with a mix of required and optional fields.
#[group]
pub struct PartialPoint {
    pub x: i64, // required: NOT NULL in required group, nullable in optional group
    pub label: Option<String>, // always nullable regardless of group embedding
}

// ── A table with both kinds of column group ───────────────────────────────────

#[table("posts")]
#[index(title)]
#[index(id, title, unique = true)]
pub struct Post {
    #[primary_key]
    pub id: BlobId,
    pub title: String,
    #[group]
    pub author: AuthorInfo,
    #[group]
    pub sig: Option<Sig>,
}

// ── Table embedding a mixed group both ways ───────────────────────────────────

#[table("partial_things")]
pub struct PartialThing {
    #[primary_key]
    pub id: i64,
    #[group]
    pub point: PartialPoint, // required group
    #[group]
    pub opt_point: Option<PartialPoint>, // optional group
}

// ── Adapter (via) types ───────────────────────────────────────────────────────

#[derive(Clone, PartialEq, Debug)]
pub enum Value {
    Number(i64),
    Text(String),
}

#[group]
pub struct ValueRaw {
    pub number: Option<i64>,
    pub text: Option<String>,
}

impl From<Value> for ValueRaw {
    fn from(v: Value) -> Self {
        match v {
            Value::Number(n) => Self {
                number: Some(n),
                text: None,
            },
            Value::Text(s) => Self {
                number: None,
                text: Some(s),
            },
        }
    }
}

impl TryFrom<ValueRaw> for Value {
    type Error = rusqlite::Error;

    fn try_from(raw: ValueRaw) -> rusqlite::Result<Self> {
        match (raw.number, raw.text) {
            (Some(n), None) => Ok(Self::Number(n)),
            (None, Some(s)) => Ok(Self::Text(s)),
            _ => Err(rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(FromSqlError::InvalidType),
            )),
        }
    }
}

#[table("entries")]
pub struct Entry {
    #[primary_key]
    pub id: i64,
    #[group(via = ValueRaw)]
    pub value: Value,
}

// ── Table using derived-column newtypes and enums ─────────────────────────────

#[table("members")]
pub struct Member {
    #[primary_key]
    pub id: i64,
    pub karma: Karma,
    pub label: Label,
}

#[table("accounts")]
pub struct Account {
    #[primary_key]
    pub id: i64,
    pub status: Status,
    pub flags: Option<bool>,
    pub prev_status: Option<Status>,
}

// ── Table with a nullable column ──────────────────────────────────────────────

#[table("events")]
pub struct Event {
    #[primary_key]
    pub id: i64,
    pub description: Option<String>,
    pub payload: Option<Vec<u8>>,
}

// ── Fixed-size byte array column ──────────────────────────────────────────────

#[table("fingerprints")]
pub struct Fingerprint {
    #[primary_key]
    pub id: i64,
    pub digest: [u8; 4],
}

// ── Compound PK table (mirrors CommitRootTree) ────────────────────────────────

#[table("commit_root_trees")]
pub struct CommitRootTree {
    #[primary_key]
    pub commit_id: BlobId,
    #[primary_key]
    pub position: i64,
    #[primary_key]
    pub is_remove: bool,
    pub tree_id: BlobId,
    pub conflict_label: String,
}

// ── DB setup ──────────────────────────────────────────────────────────────────

fn setup() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        &[
            Widget::CREATE_TABLE,
            Post::CREATE_TABLE,
            Entry::CREATE_TABLE,
            Member::CREATE_TABLE,
            Account::CREATE_TABLE,
            Event::CREATE_TABLE,
            PartialThing::CREATE_TABLE,
            Fingerprint::CREATE_TABLE,
            CommitRootTree::CREATE_TABLE,
        ]
        .concat(),
    )
    .unwrap();
    conn
}

// ── Schema constant tests ─────────────────────────────────────────────────────

#[test]
fn schema_constant_concatenates_create_tables() {
    assert!(catalog::SCHEMA.contains("CREATE TABLE IF NOT EXISTS alpha"));
    assert!(catalog::SCHEMA.contains("CREATE TABLE IF NOT EXISTS beta"));
    assert_eq!(
        catalog::SCHEMA,
        [catalog::Alpha::CREATE_TABLE, catalog::Beta::CREATE_TABLE].concat(),
    );

    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(catalog::SCHEMA).unwrap();
}

// ── SQL constant tests ────────────────────────────────────────────────────────

#[test]
fn widget_constants() {
    assert_eq!(Widget::SELECT, "SELECT id, name FROM widgets");
    assert_eq!(
        Widget::INSERT,
        "INSERT INTO widgets (id, name) VALUES (:id, :name) ON CONFLICT DO NOTHING"
    );
    assert!(Widget::CREATE_TABLE.contains("CREATE TABLE IF NOT EXISTS widgets"));
    assert!(Widget::CREATE_TABLE.contains("id BLOB NOT NULL"));
    assert!(Widget::CREATE_TABLE.contains("PRIMARY KEY (id)"));
    assert!(Widget::CREATE_TABLE.contains("name TEXT NOT NULL"));
    assert!(
        Widget::CREATE_TABLE
            .contains("CREATE INDEX IF NOT EXISTS idx_widgets_name ON widgets (name)")
    );
}

#[test]
fn post_schema_constants() {
    assert!(Post::CREATE_TABLE.contains("title TEXT NOT NULL"));
    assert!(Post::CREATE_TABLE.contains("author_name TEXT NOT NULL"));
    assert!(Post::CREATE_TABLE.contains("author_karma INTEGER NOT NULL"));
    // Optional group: columns are nullable (no NOT NULL).
    assert!(Post::CREATE_TABLE.contains("sig_data BLOB"));
    assert!(!Post::CREATE_TABLE.contains("sig_data BLOB NOT NULL"));
    assert!(Post::CREATE_TABLE.contains("sig_hash BLOB"));
    assert!(!Post::CREATE_TABLE.contains("sig_hash BLOB NOT NULL"));

    assert!(Post::SELECT.contains("author_name"));
    assert!(Post::SELECT.contains("author_karma"));
    assert!(Post::SELECT.contains("sig_data"));
    assert!(Post::SELECT.contains("sig_hash"));

    assert!(Post::INSERT.contains(":author_name"));
    assert!(Post::INSERT.contains(":sig_data"));
    assert!(
        Post::CREATE_TABLE.contains("CREATE INDEX IF NOT EXISTS idx_posts_title ON posts (title)")
    );
    assert!(
        Post::CREATE_TABLE
            .contains("CREATE UNIQUE INDEX IF NOT EXISTS idx_posts_id_title ON posts (id, title)")
    );
}

#[test]
fn commit_root_tree_compound_pk() {
    assert!(CommitRootTree::CREATE_TABLE.contains("PRIMARY KEY (commit_id, position, is_remove)"));
    assert_eq!(
        CommitRootTree::SELECT,
        "SELECT commit_id, position, is_remove, tree_id, conflict_label FROM commit_root_trees"
    );
}

// ── Entry (via adapter) tests ─────────────────────────────────────────────────

#[test]
fn entry_schema_uses_raw_type_columns() {
    // Both columns come from ValueRaw; they are nullable because Option<_>.
    assert!(Entry::CREATE_TABLE.contains("value_number INTEGER"));
    assert!(!Entry::CREATE_TABLE.contains("value_number INTEGER NOT NULL"));
    assert!(Entry::CREATE_TABLE.contains("value_text TEXT"));
    assert!(!Entry::CREATE_TABLE.contains("value_text TEXT NOT NULL"));
    assert!(Entry::SELECT.contains("value_number"));
    assert!(Entry::SELECT.contains("value_text"));
    assert!(Entry::INSERT.contains(":value_number"));
    assert!(Entry::INSERT.contains(":value_text"));
}

#[test]
fn entry_via_roundtrip() {
    let conn = setup();
    insert(
        &conn,
        Entry {
            id: 1,
            value: Value::Number(42),
        },
    )
    .unwrap();
    insert(
        &conn,
        Entry {
            id: 2,
            value: Value::Text("hello".to_owned()),
        },
    )
    .unwrap();

    let fetch = |id: i64| {
        conn.prepare_cached(&format!("{} WHERE id = ?1", Entry::SELECT))
            .unwrap()
            .query_row((id,), Entry::from_row)
            .unwrap()
    };

    assert_eq!(fetch(1).value, Value::Number(42));
    assert_eq!(fetch(2).value, Value::Text("hello".to_owned()));
}

// ── Widget roundtrip tests ────────────────────────────────────────────────────

#[test]
fn widget_roundtrip() {
    let conn = setup();
    insert(
        &conn,
        Widget {
            id: BlobId::new(1),
            name: "sprocket".to_owned(),
        },
    )
    .unwrap();
    let got = conn
        .prepare_cached(&format!("{} WHERE id = ?1", Widget::SELECT))
        .unwrap()
        .query_row((&BlobId::new(1),), Widget::from_row)
        .unwrap();
    assert_eq!(got.id, BlobId::new(1));
    assert_eq!(got.name, "sprocket");
}

#[test]
fn widget_insert_do_nothing() {
    let conn = setup();
    insert(
        &conn,
        Widget {
            id: BlobId::new(2),
            name: "first".to_owned(),
        },
    )
    .unwrap();
    insert(
        &conn,
        Widget {
            id: BlobId::new(2),
            name: "second".to_owned(),
        },
    )
    .unwrap();
    let got = conn
        .prepare_cached(&format!("{} WHERE id = ?1", Widget::SELECT))
        .unwrap()
        .query_row((&BlobId::new(2),), Widget::from_row)
        .unwrap();
    assert_eq!(got.name, "first");
}

// ── Post roundtrip tests ──────────────────────────────────────────────────────

fn make_post(id: u8, with_sig: bool) -> Post {
    Post {
        id: BlobId::new(id),
        title: "Hello".to_owned(),
        author: AuthorInfo {
            name: "Alice".to_owned(),
            karma: 42,
        },
        sig: if with_sig {
            Some(Sig {
                data: vec![0xde, 0xad],
                hash: vec![0xbe, 0xef],
            })
        } else {
            None
        },
    }
}

fn fetch_post(conn: &Connection, id: u8) -> Post {
    conn.prepare_cached(&format!("{} WHERE id = ?1", Post::SELECT))
        .unwrap()
        .query_row((&BlobId::new(id),), Post::from_row)
        .unwrap()
}

#[test]
fn post_roundtrip_with_sig() {
    let conn = setup();
    insert(&conn, make_post(1, true)).unwrap();
    let got = fetch_post(&conn, 1);

    assert_eq!(got.id, BlobId::new(1));
    assert_eq!(got.title, "Hello");
    assert_eq!(got.author.name, "Alice");
    assert_eq!(got.author.karma, 42);

    let sig = got.sig.expect("sig should be Some");
    assert_eq!(sig.data, vec![0xde, 0xad]);
    assert_eq!(sig.hash, vec![0xbe, 0xef]);
}

#[test]
fn post_roundtrip_without_sig() {
    let conn = setup();
    insert(&conn, make_post(2, false)).unwrap();
    let got = fetch_post(&conn, 2);

    assert_eq!(got.id, BlobId::new(2));
    assert_eq!(got.author.name, "Alice");
    assert!(got.sig.is_none());
}

#[test]
fn post_optional_sig_none_to_some_distinction() {
    let conn = setup();
    insert(&conn, make_post(3, true)).unwrap();
    insert(&conn, make_post(4, false)).unwrap();

    assert!(fetch_post(&conn, 3).sig.is_some());
    assert!(fetch_post(&conn, 4).sig.is_none());
}

// ── derive(Column) enum tests ─────────────────────────────────────────────────

#[test]
fn enum_column_ddl() {
    // Required enum column: CHECK constraint with NOT NULL.
    assert!(Account::CREATE_TABLE.contains("status INTEGER CHECK (status IN (1, 2, 3)) NOT NULL"));
    // Optional enum column: CHECK constraint, no NOT NULL.
    assert!(Account::CREATE_TABLE.contains("prev_status INTEGER CHECK (prev_status IN (1, 2, 3))"));
    assert!(
        !Account::CREATE_TABLE
            .contains("prev_status INTEGER CHECK (prev_status IN (1, 2, 3)) NOT NULL")
    );
    // Optional bool column: CHECK constraint, no NOT NULL.
    assert!(Account::CREATE_TABLE.contains("flags INTEGER CHECK (flags IN (0, 1))"));
    assert!(!Account::CREATE_TABLE.contains("flags INTEGER CHECK (flags IN (0, 1)) NOT NULL"));
}

#[test]
fn enum_column_roundtrip() {
    let conn = setup();
    insert(
        &conn,
        Account {
            id: 1,
            status: Status::Active,
            flags: Some(true),
            prev_status: None,
        },
    )
    .unwrap();
    insert(
        &conn,
        Account {
            id: 2,
            status: Status::Deleted,
            flags: None,
            prev_status: Some(Status::Active),
        },
    )
    .unwrap();

    let r1 = Account::get(&conn, &1i64).unwrap();
    assert_eq!(r1.status, Status::Active);
    assert_eq!(r1.flags, Some(true));
    assert_eq!(r1.prev_status, None);

    let r2 = Account::get(&conn, &2i64).unwrap();
    assert_eq!(r2.status, Status::Deleted);
    assert_eq!(r2.flags, None);
    assert_eq!(r2.prev_status, Some(Status::Active));
}

#[test]
fn bool_column_has_check_constraint() {
    // Required bool: CHECK constraint AND NOT NULL.
    assert!(
        CommitRootTree::CREATE_TABLE
            .contains("is_remove INTEGER CHECK (is_remove IN (0, 1)) NOT NULL")
    );
}

// ── derive(Column) newtype tests ──────────────────────────────────────────────

#[test]
fn newtype_column_roundtrip() {
    let conn = setup();
    insert(
        &conn,
        Member {
            id: 1,
            karma: Karma(42),
            label: Label("founder".to_owned()),
        },
    )
    .unwrap();

    let got = Member::get(&conn, &1i64).unwrap();
    assert_eq!(got.karma, Karma(42));
    assert_eq!(got.label, Label("founder".to_owned()));
}

#[test]
fn newtype_column_sql_type_inherited() {
    assert_eq!(Karma::SQL_TYPE, i64::SQL_TYPE);
    assert_eq!(Label::SQL_TYPE, String::SQL_TYPE);
}

// ── Group nullability tests ───────────────────────────────────────────────────

#[test]
fn group_nullability_ddl() {
    // Required group: non-optional field is NOT NULL, optional field is nullable.
    assert!(PartialThing::CREATE_TABLE.contains("point_x INTEGER NOT NULL"));
    assert!(PartialThing::CREATE_TABLE.contains("point_label TEXT"));
    assert!(!PartialThing::CREATE_TABLE.contains("point_label TEXT NOT NULL"));

    // Optional group: all fields are nullable regardless of their own type.
    assert!(PartialThing::CREATE_TABLE.contains("opt_point_x INTEGER"));
    assert!(!PartialThing::CREATE_TABLE.contains("opt_point_x INTEGER NOT NULL"));
    assert!(PartialThing::CREATE_TABLE.contains("opt_point_label TEXT"));
    assert!(!PartialThing::CREATE_TABLE.contains("opt_point_label TEXT NOT NULL"));
}

#[test]
fn group_nullability_roundtrip() {
    let conn = setup();

    // Required group present, optional group absent.
    insert(
        &conn,
        PartialThing {
            id: 1,
            point: PartialPoint {
                x: 10,
                label: Some("hello".to_owned()),
            },
            opt_point: None,
        },
    )
    .unwrap();

    // Required group, optional field within it is None.
    insert(
        &conn,
        PartialThing {
            id: 2,
            point: PartialPoint { x: 20, label: None },
            opt_point: Some(PartialPoint {
                x: 30,
                label: Some("world".to_owned()),
            }),
        },
    )
    .unwrap();

    let r1 = PartialThing::get(&conn, &1i64).unwrap();
    assert_eq!(r1.point.x, 10);
    assert_eq!(r1.point.label, Some("hello".to_owned()));
    assert!(r1.opt_point.is_none());

    let r2 = PartialThing::get(&conn, &2i64).unwrap();
    assert_eq!(r2.point.x, 20);
    assert_eq!(r2.point.label, None);
    let op = r2.opt_point.unwrap();
    assert_eq!(op.x, 30);
    assert_eq!(op.label, Some("world".to_owned()));
}

// ── Option<T> column tests ────────────────────────────────────────────────────

#[test]
fn nullable_column_ddl() {
    assert!(Event::CREATE_TABLE.contains("id INTEGER NOT NULL"));
    assert!(Event::CREATE_TABLE.contains("description TEXT"));
    assert!(!Event::CREATE_TABLE.contains("description TEXT NOT NULL"));
    assert!(Event::CREATE_TABLE.contains("payload BLOB"));
    assert!(!Event::CREATE_TABLE.contains("payload BLOB NOT NULL"));
}

#[test]
fn nullable_via_type_alias() {
    // NULLABLE is evaluated by the compiler, so it resolves type aliases —
    // this would fail with the old syntactic Option<T> check.
    type MaybeText = Option<String>;
    const {
        assert!(<MaybeText as balsaq::Column>::NULLABLE);
    }
    assert_eq!(<MaybeText as balsaq::Column>::SQL_TYPE, "TEXT");
}

#[test]
fn nullable_column_roundtrip() {
    let conn = setup();
    insert(
        &conn,
        Event {
            id: 1,
            description: Some("hello".to_owned()),
            payload: None,
        },
    )
    .unwrap();
    insert(
        &conn,
        Event {
            id: 2,
            description: None,
            payload: Some(vec![0xaa]),
        },
    )
    .unwrap();

    let got1 = Event::get(&conn, &1i64).unwrap();
    assert_eq!(got1.description, Some("hello".to_owned()));
    assert_eq!(got1.payload, None);

    let got2 = Event::get(&conn, &2i64).unwrap();
    assert_eq!(got2.description, None);
    assert_eq!(got2.payload, Some(vec![0xaa]));
}

// ── get() tests ───────────────────────────────────────────────────────────────

#[test]
fn get_by_single_pk() {
    let conn = setup();
    insert(
        &conn,
        Widget {
            id: BlobId::new(7),
            name: "bolt".to_owned(),
        },
    )
    .unwrap();

    let found = Widget::get(&conn, &BlobId::new(7)).unwrap();
    assert_eq!(found.name, "bolt");

    assert!(matches!(
        Widget::get(&conn, &BlobId::new(99)),
        Err(rusqlite::Error::QueryReturnedNoRows)
    ));
}

#[test]
fn get_by_compound_pk() {
    let conn = setup();
    insert(
        &conn,
        CommitRootTree {
            commit_id: BlobId::new(9),
            position: 2,
            is_remove: true,
            tree_id: BlobId::new(42),
            conflict_label: "left".to_owned(),
        },
    )
    .unwrap();

    let got = CommitRootTree::get(&conn, (&BlobId::new(9), &2i64, &true)).unwrap();
    assert_eq!(got.tree_id, BlobId::new(42));
    assert_eq!(got.conflict_label, "left");

    assert!(matches!(
        CommitRootTree::get(&conn, (&BlobId::new(9), &2i64, &false)),
        Err(rusqlite::Error::QueryReturnedNoRows)
    ));
}

// ── [u8; N] column tests ──────────────────────────────────────────────────────

#[test]
fn fixed_bytes_column_ddl() {
    assert!(Fingerprint::CREATE_TABLE.contains("digest BLOB NOT NULL"));
}

#[test]
fn fixed_bytes_column_roundtrip() {
    let conn = setup();
    insert(
        &conn,
        Fingerprint {
            id: 1,
            digest: [0xde, 0xad, 0xbe, 0xef],
        },
    )
    .unwrap();
    let got = Fingerprint::get(&conn, &1i64).unwrap();
    assert_eq!(got.digest, [0xde, 0xad, 0xbe, 0xef]);
}

// ── CommitRootTree roundtrip tests ────────────────────────────────────────────

#[test]
fn commit_root_tree_roundtrip() {
    let conn = setup();
    let rows = vec![
        CommitRootTree {
            commit_id: BlobId::new(1),
            position: 0,
            is_remove: false,
            tree_id: BlobId::new(10),
            conflict_label: String::new(),
        },
        CommitRootTree {
            commit_id: BlobId::new(1),
            position: 1,
            is_remove: true,
            tree_id: BlobId::new(11),
            conflict_label: "base".to_owned(),
        },
    ];
    for row in rows {
        insert(&conn, row).unwrap();
    }
    let got: Vec<CommitRootTree> = conn
        .prepare_cached(&format!(
            "{} WHERE commit_id = ?1 ORDER BY is_remove ASC, position ASC",
            CommitRootTree::SELECT
        ))
        .unwrap()
        .query_map((&BlobId::new(1),), CommitRootTree::from_row)
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();

    assert_eq!(got.len(), 2);
    assert_eq!(got[0].tree_id, BlobId::new(10));
    assert!(!got[0].is_remove);
    assert_eq!(got[1].tree_id, BlobId::new(11));
    assert!(got[1].is_remove);
    assert_eq!(got[1].conflict_label, "base");
}
