mod derive_column;
mod expand_group;
mod expand_schema;
mod expand_table;

use proc_macro::TokenStream;

/// Derives [`balsaq::Column`], [`rusqlite::types::ToSql`], and [`rusqlite::types::FromSql`]. Works
/// on:
///
/// - **Single-field tuple structs** — delegates all three impls to the inner type, which must
///   itself implement all three.
/// - **C-like enums with `#[repr(integer)]`** — stores the discriminant as an `INTEGER` and
///   generates a `CHECK ({col} IN (...))` constraint from the known discriminant values.
///
/// # Usage
///
/// ```rust,ignore
/// #[derive(balsaq::Column, Clone, Copy, Debug, PartialEq)]
/// pub struct UserId(i64);
///
/// #[derive(balsaq::Column, Clone, Copy, Debug, PartialEq)]
/// #[repr(i64)]
/// pub enum Status { Active = 1, Inactive = 2 }
/// ```
#[proc_macro_derive(Column)]
pub fn derive_column(item: TokenStream) -> TokenStream {
    match derive_column::expand(item.into()) {
        Ok(ts) => ts.into(),
        Err(e) => e.into_compile_error().into(),
    }
}

/// Derives `Model` for a struct whose fields map 1-to-1 to SQL columns or column sets.
///
/// # Usage
///
/// ```rust,ignore
/// #[balsaq::table("my_table")]
/// pub struct MyRow {
///     #[column("BLOB NOT NULL", primary_key = true)]
///     pub id: MyId,
///     #[column("TEXT NOT NULL")]
///     pub name: String,
///     #[column_set]
///     pub author: AuthorInfo,          // AuthorInfo: ColumnSet
///     #[column_set(optional)]
///     pub sig: Option<SigInfo>,        // SigInfo: ColumnSetOptional
/// }
/// ```
#[proc_macro_attribute]
pub fn table(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attr2 = proc_macro2::TokenStream::from(attr);
    let item2 = proc_macro2::TokenStream::from(item);
    match expand_table::expand(attr2, item2) {
        Ok(ts) => ts.into(),
        Err(e) => e.into_compile_error().into(),
    }
}

/// Collects all `#[table]` structs in a module and generates a `pub const SCHEMA: &'static str`
/// that is their `CREATE_TABLE` strings concatenated in declaration order. Pass it directly to
/// `conn.execute_batch(mymod::SCHEMA)` to set up the whole schema.
///
/// # Usage
///
/// ```rust,ignore
/// #[balsaq::schema]
/// pub mod db {
///     use balsaq::table;
///
///     #[table("widgets")]
///     pub struct Widget { /* … */ }
///
///     #[table("posts")]
///     pub struct Post { /* … */ }
/// }
///
/// // Elsewhere:
/// conn.execute_batch(db::SCHEMA)?;
/// ```
#[proc_macro_attribute]
pub fn schema(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attr2 = proc_macro2::TokenStream::from(attr);
    let item2 = proc_macro2::TokenStream::from(item);
    match expand_schema::expand(attr2, item2) {
        Ok(ts) => ts.into(),
        Err(e) => e.into_compile_error().into(),
    }
}

/// Derives [`balsaq::ColumnGroup`] for a struct whose fields each map to a single SQL column, in
/// declaration order. Field types must implement [`balsaq::Column`] — no per-field annotations
/// are needed.
///
/// A `#[group]` struct can be embedded as a required group (`pub author: Sig`) or as an optional
/// group (`pub sig: Option<Sig>`) in any `#[table]` struct. No extra annotation is required for the
/// optional case.
///
/// # Usage
///
/// ```rust,ignore
/// #[balsaq::group]
/// pub struct Signature {
///     pub name: String,
///     pub timestamp: i64,
/// }
/// ```
#[proc_macro_attribute]
pub fn group(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attr2 = proc_macro2::TokenStream::from(attr);
    let item2 = proc_macro2::TokenStream::from(item);
    if !attr2.is_empty() {
        return syn::Error::new_spanned(attr2, "#[group] takes no arguments")
            .into_compile_error()
            .into();
    }
    match expand_group::expand(item2) {
        Ok(ts) => ts.into(),
        Err(e) => e.into_compile_error().into(),
    }
}
