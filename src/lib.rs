pub use balsaq_macros::{Column, group, schema, table};
pub use rusqlite;

// Re-exported so generated code can reference it via ::balsaq::__cf::concatcp! etc.
#[doc(hidden)]
pub use const_format as __cf;

use rusqlite::types::{FromSql, ToSql};
use rusqlite::{CachedStatement, Connection, Row};

/// A type that maps to a single SQL column. Implementing this trait lets the type be used as a
/// plain (unannotated) field in a `#[table]` or `#[group]` struct — the macro reads `SQL_TYPE` to
/// build the `CREATE TABLE` DDL.
///
/// `SQL_TYPE` holds only the base SQL type (e.g. `"TEXT"`, `"BLOB"`) without a nullability
/// qualifier. The macro appends ` NOT NULL` for non-`Option` fields and omits it for `Option<T>`
/// fields, so nullability is expressed entirely through the Rust type.
///
/// `SQL_TYPE` may contain the placeholder `{col}`, which the `#[table]` and `#[group]` macros
/// substitute with the actual column name at the use site. This is useful for CHECK constraints
/// that reference the column by name, e.g. `"INTEGER CHECK ({col} IN (0, 1))"` for `bool`.
///
/// balsaq provides impls for the common Rust primitives. Custom types can implement it manually
/// or get it for free via `#[derive(balsaq::Column)]`, which works on single-field tuple structs
/// (inferring `SQL_TYPE` from the inner field) and C-like enums with `#[repr(integer)]` (generating
/// an `INTEGER CHECK ({col} IN (...))` constraint from the discriminant values).
pub trait Column: ToSql + FromSql {
    const SQL_TYPE: &'static str;
    /// `true` if this type maps to a nullable SQL column, `false` otherwise. The macro uses this to
    /// decide whether to append ` NOT NULL` to the DDL.
    const NULLABLE: bool;
}

impl Column for String {
    const SQL_TYPE: &'static str = "TEXT";
    const NULLABLE: bool = false;
}
impl Column for i64 {
    const SQL_TYPE: &'static str = "INTEGER";
    const NULLABLE: bool = false;
}
impl Column for i32 {
    const SQL_TYPE: &'static str = "INTEGER";
    const NULLABLE: bool = false;
}
impl Column for f64 {
    const SQL_TYPE: &'static str = "REAL";
    const NULLABLE: bool = false;
}
impl Column for Vec<u8> {
    const SQL_TYPE: &'static str = "BLOB";
    const NULLABLE: bool = false;
}
impl Column for bool {
    const SQL_TYPE: &'static str = "INTEGER CHECK ({col} IN (0, 1))";
    const NULLABLE: bool = false;
}

impl<const N: usize> Column for [u8; N] {
    const SQL_TYPE: &'static str = "BLOB";
    const NULLABLE: bool = false;
}

impl<T: Column> Column for Option<T> {
    const SQL_TYPE: &'static str = T::SQL_TYPE;
    const NULLABLE: bool = true;
}

/// Returns `" NOT NULL"` for required columns, `""` for nullable ones. Used by generated code — not
/// part of the public API.
#[doc(hidden)]
pub const fn __null_qualifier(nullable: bool) -> &'static str {
    if nullable { "" } else { " NOT NULL" }
}

/// Returns `"{NN}"` for fields whose nullability is controlled by the enclosing group (substituted
/// at the use site), `""` for fields that are always nullable regardless of group optionality. Used
/// by generated code — not part of the public API.
#[doc(hidden)]
pub const fn __nn_placeholder(nullable: bool) -> &'static str {
    if nullable { "" } else { "{NN}" }
}

pub trait Model: Sized {
    const CREATE_TABLE: &'static str;
    const SELECT: &'static str;
    const INSERT: &'static str;

    /// The primary key type, parameterised by a lifetime so it is always a reference or tuple of
    /// references — callers never need to own a PK value. For tables with no primary key this is
    /// `&'pk std::convert::Infallible`, making `get` uncallable.
    type PrimaryKey<'pk>;

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self>;
    fn write_params(self, stmt: &mut CachedStatement<'_>) -> rusqlite::Result<usize>;

    fn get<'pk>(conn: &Connection, pk: Self::PrimaryKey<'pk>) -> rusqlite::Result<Self>;
}

pub fn insert<T: Model>(conn: &Connection, value: T) -> rusqlite::Result<()> {
    value.write_params(&mut conn.prepare_cached(T::INSERT)?)?;
    Ok(())
}

pub fn get_all<T: Model, P: rusqlite::Params>(
    conn: &Connection,
    sql: &'static str,
    params: P,
) -> rusqlite::Result<Vec<T>> {
    conn.prepare_cached(sql)?
        .query_map(params, T::from_row)?
        .collect()
}

/// A group of SQL columns that always appear together under a shared prefix, in declaration order.
/// Generated automatically by `#[group]` on a struct.
///
/// The string constants use `{P}` as a placeholder for the column prefix and `{NN}` for the
/// nullability qualifier (` NOT NULL` or `""`), both substituted by the outer `#[table]` macro at
/// the use site.
///
/// `impl<T: ColumnGroup> ColumnGroup for Option<T>` is provided by balsaq, so a field typed
/// `Option<Sig>` in a `#[table]` struct is automatically an optional group — no extra annotation
/// needed.
pub trait ColumnGroup: Sized {
    /// DDL lines for CREATE TABLE, with `{P}` and `{NN}` placeholders. e.g.
    /// `"    {P}name TEXT{NN},\n    {P}email TEXT{NN}"`
    const DDL: &'static str;
    /// Comma-separated column names for SELECT/INSERT, with `{P}`.
    const COLS: &'static str;
    /// Comma-separated named params for VALUES, with `{P}`.
    const VALS: &'static str;
    /// `true` for `Option<T>` — tells the outer `#[table]` macro to substitute `{NN}` with `""`
    /// instead of `" NOT NULL"`.
    const NULLABLE: bool;

    fn cg_read(row: &Row<'_>, prefix: &str) -> rusqlite::Result<Self>;
    fn cg_write<'a>(&'a self, prefix: &str, out: &mut Vec<(String, &'a dyn ToSql)>);

    /// Read `Option<Self>` by checking whether every column is SQL NULL. Returns `None` when all
    /// columns are NULL, `Some(Self)` otherwise. Generated by `#[group]`; used internally by `impl
    /// ColumnGroup for Option<T>`.
    fn cg_read_optional(row: &Row<'_>, prefix: &str) -> rusqlite::Result<Option<Self>>;

    /// Push SQL NULL for every column of this group. Generated by `#[group]`; used internally by
    /// `impl ColumnGroup for Option<T>`.
    fn cg_write_null(prefix: &str, out: &mut Vec<(String, &dyn ToSql)>);
}

impl<T: ColumnGroup> ColumnGroup for Option<T> {
    // {NN} substitution is handled by the outer #[table] macro, which reads NULLABLE = true and
    // replaces {NN} with "" (making columns nullable).
    const DDL: &'static str = T::DDL;
    const COLS: &'static str = T::COLS;
    const VALS: &'static str = T::VALS;
    const NULLABLE: bool = true;

    fn cg_read(row: &Row<'_>, prefix: &str) -> rusqlite::Result<Self> {
        T::cg_read_optional(row, prefix)
    }

    fn cg_write<'a>(&'a self, prefix: &str, out: &mut Vec<(String, &'a dyn ToSql)>) {
        match self {
            Some(t) => t.cg_write(prefix, out),
            None => T::cg_write_null(prefix, out),
        }
    }

    fn cg_read_optional(row: &Row<'_>, prefix: &str) -> rusqlite::Result<Option<Self>> {
        T::cg_read_optional(row, prefix).map(Some)
    }

    fn cg_write_null(prefix: &str, out: &mut Vec<(String, &dyn ToSql)>) {
        T::cg_write_null(prefix, out)
    }
}
