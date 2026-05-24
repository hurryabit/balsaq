use proc_macro2::TokenStream;
use quote::quote;
use syn::{
    Error, Fields, ItemStruct, LitBool, LitStr, Meta, Result, Token,
    parse::{Parse, ParseStream},
    spanned::Spanned,
};

// Parses the argument list inside a field-level #[group(...)] attribute:
//   (no args)      → Plain  (Meta::Path, handled before reaching this parser)
//   via = SomeType → Via(SomeType)
#[allow(clippy::large_enum_variant)]
enum FieldGroupArgs {
    Plain,
    Via(syn::Type),
}

impl Parse for FieldGroupArgs {
    fn parse(input: ParseStream) -> Result<Self> {
        let kw: syn::Ident = input.parse()?;
        if kw != "via" {
            return Err(Error::new(kw.span(), "expected `via = Type`"));
        }
        let _: Token![=] = input.parse()?;
        let ty: syn::Type = input.parse()?;
        if !input.is_empty() {
            return Err(Error::new(
                input.span(),
                "#[group(via = ...)] takes no further arguments",
            ));
        }
        Ok(Self::Via(ty))
    }
}

struct TableArgs {
    table_name: LitStr,
}

impl Parse for TableArgs {
    fn parse(input: ParseStream) -> Result<Self> {
        let table_name: LitStr = input.parse()?;
        Ok(Self { table_name })
    }
}

// Parses:  col1
//       or col1, col2
//       or col1, col2, unique = true
struct IndexArgs {
    columns: Vec<syn::Ident>,
    unique: bool,
}

impl Parse for IndexArgs {
    fn parse(input: ParseStream) -> Result<Self> {
        let mut columns: Vec<syn::Ident> = Vec::new();
        let mut unique = false;
        let mut first = true;

        while !input.is_empty() {
            if !first {
                let _: Token![,] = input.parse()?;
                if input.is_empty() {
                    break; // tolerate trailing comma
                }
            }
            first = false;

            let ident: syn::Ident = input.parse()?;
            if ident == "unique" && input.peek(Token![=]) {
                let _: Token![=] = input.parse()?;
                let val: LitBool = input.parse()?;
                unique = val.value;
            } else {
                columns.push(ident);
            }
        }

        if columns.is_empty() {
            return Err(Error::new(
                proc_macro2::Span::call_site(),
                "#[index] requires at least one column name",
            ));
        }

        Ok(Self { columns, unique })
    }
}

#[allow(clippy::large_enum_variant)]
enum FieldKind {
    /// An unannotated field or `#[primary_key]` — type implements `Column`.
    Column { primary_key: bool, ty: syn::Type },
    /// A `#[group]` field — type implements `ColumnGroup`. Covers both required (`Sig`) and
    /// optional (`Option<Sig>`) groups; `ColumnGroup::NULLABLE` drives the `{NN}` substitution.
    ColumnGroup { ty: syn::Type, prefix: String },
    /// A `#[group(via = RawType)]` field — the SQL layer uses `RawType` (which implements
    /// `ColumnGroup`); the domain field type converts via `TryFrom<RawType>` (read) and
    /// `From<FieldType>` (write).
    ColumnGroupVia {
        raw_ty: syn::Type,
        field_ty: syn::Type,
        prefix: String,
    },
}

struct FieldInfo {
    ident: syn::Ident,
    kind: FieldKind,
}

pub(crate) fn expand(attr: TokenStream, item: TokenStream) -> Result<TokenStream> {
    let TableArgs { table_name } = syn::parse2::<TableArgs>(attr)?;
    let table = table_name.value();

    let mut item_struct: ItemStruct = syn::parse2(item)?;
    let struct_name = item_struct.ident.clone();

    // Collect and strip #[index] and #[track_last_update] attrs from the struct before re-emitting it.
    let mut indices: Vec<IndexArgs> = Vec::new();
    let mut track_last_update = false;
    let mut kept_attrs = Vec::new();
    for attr in item_struct.attrs.drain(..) {
        if attr.path().is_ident("index") {
            indices.push(attr.parse_args::<IndexArgs>()?);
        } else if attr.path().is_ident("track_last_update") {
            if !matches!(attr.meta, Meta::Path(_)) {
                return Err(Error::new(
                    attr.span(),
                    "#[track_last_update] takes no arguments",
                ));
            }
            track_last_update = true;
        } else {
            kept_attrs.push(attr);
        }
    }
    item_struct.attrs = kept_attrs;

    let Fields::Named(ref mut named) = item_struct.fields else {
        return Err(Error::new(
            struct_name.span(),
            "#[table] only supports structs with named fields",
        ));
    };

    let mut fields: Vec<FieldInfo> = Vec::new();

    for field in named.named.iter_mut() {
        let ident = field.ident.clone().expect("named field has ident");
        let prefix = format!("{}_", ident);

        let group_pos = field.attrs.iter().position(|a| a.path().is_ident("group"));
        let pk_pos = field
            .attrs
            .iter()
            .position(|a| a.path().is_ident("primary_key"));

        let kind = if let Some(group_pos) = group_pos {
            if let Some(pk_pos) = pk_pos {
                return Err(Error::new(
                    field.attrs[pk_pos].span(),
                    "a field cannot be both #[group] and #[primary_key]",
                ));
            }
            let group_attr = field.attrs.remove(group_pos);
            let args = match &group_attr.meta {
                Meta::Path(_) => FieldGroupArgs::Plain,
                Meta::List(list) => syn::parse2::<FieldGroupArgs>(list.tokens.clone())?,
                Meta::NameValue(_) => {
                    return Err(Error::new(group_attr.span(), "unexpected attribute form"));
                }
            };
            match args {
                FieldGroupArgs::Plain => FieldKind::ColumnGroup {
                    ty: field.ty.clone(),
                    prefix,
                },
                FieldGroupArgs::Via(raw_ty) => FieldKind::ColumnGroupVia {
                    raw_ty,
                    field_ty: field.ty.clone(),
                    prefix,
                },
            }
        } else if let Some(pk_pos) = pk_pos {
            field.attrs.remove(pk_pos);
            FieldKind::Column {
                primary_key: true,
                ty: field.ty.clone(),
            }
        } else {
            FieldKind::Column {
                primary_key: false,
                ty: field.ty.clone(),
            }
        };

        fields.push(FieldInfo { ident, kind });
    }

    // Validate #[index] column names
    let plain_field_names: std::collections::HashSet<String> = fields
        .iter()
        .filter_map(|f| {
            if matches!(f.kind, FieldKind::Column { .. }) {
                Some(f.ident.to_string())
            } else {
                None
            }
        })
        .collect();

    for idx in &indices {
        for col_ident in &idx.columns {
            let col_name = col_ident.to_string();
            if !plain_field_names.contains(&col_name) {
                let is_group = fields.iter().any(|f| f.ident == *col_ident);
                let msg = if is_group {
                    format!(
                        "`{col_name}` is a #[group] field; only plain column fields can be indexed"
                    )
                } else {
                    format!("unknown column `{col_name}`; not a plain field of this struct")
                };
                return Err(Error::new(col_ident.span(), msg));
            }
        }
    }

    // Schema constants: We always use concatcp! so that str_replace! invocations (needed for group
    // DDL/COLS/VALS) compose cleanly with Column::SQL_TYPE references.

    // CREATE TABLE DDL pieces
    let mut ddl_args: Vec<TokenStream> = Vec::new();
    let create_prefix = format!("CREATE TABLE IF NOT EXISTS {table} (\n");
    ddl_args.push(quote! { #create_prefix });

    for (i, f) in fields.iter().enumerate() {
        if i > 0 {
            ddl_args.push(quote! { ",\n" });
        }
        match &f.kind {
            FieldKind::Column { ty, .. } => {
                let col_name = f.ident.to_string();
                let col_prefix = format!("    {col_name} ");
                ddl_args.push(quote! { #col_prefix });
                // Substitute {col} with the field name so SQL_TYPE can embed
                // column-name-dependent CHECK constraints (e.g. bool, enums).
                ddl_args.push(quote! {
                    ::balsaq::__cf::str_replace!(
                        <#ty as ::balsaq::Column>::SQL_TYPE, "{col}", #col_name
                    )
                });
                // Column::NULLABLE is evaluated by the Rust compiler, so it
                // correctly handles type aliases (e.g. `type X = Option<i64>`).
                ddl_args.push(quote! {
                    ::balsaq::__null_qualifier(<#ty as ::balsaq::Column>::NULLABLE)
                });
            }
            FieldKind::ColumnGroup { ty, prefix } => {
                // ColumnGroup::NULLABLE drives {NN}: false → " NOT NULL", true → "".
                // Option<T>::NULLABLE = true, so optional groups get nullable columns.
                ddl_args.push(quote! {
                    ::balsaq::__cf::str_replace!(
                        ::balsaq::__cf::str_replace!(<#ty as ::balsaq::ColumnGroup>::DDL, "{P}", #prefix),
                        "{NN}", ::balsaq::__null_qualifier(<#ty as ::balsaq::ColumnGroup>::NULLABLE)
                    )
                });
            }
            FieldKind::ColumnGroupVia { raw_ty, prefix, .. } => {
                ddl_args.push(quote! {
                    ::balsaq::__cf::str_replace!(
                        ::balsaq::__cf::str_replace!(<#raw_ty as ::balsaq::ColumnGroup>::DDL, "{P}", #prefix),
                        "{NN}", " NOT NULL"
                    )
                });
            }
        }
    }

    // Collect PK fields.
    let pk_field_info: Vec<(&syn::Ident, &syn::Type)> = fields
        .iter()
        .filter_map(|f| {
            if let FieldKind::Column {
                primary_key: true,
                ty,
            } = &f.kind
            {
                Some((&f.ident, ty))
            } else {
                None
            }
        })
        .collect();

    if track_last_update {
        if pk_field_info.is_empty() {
            return Err(Error::new(
                struct_name.span(),
                "#[track_last_update] requires at least one #[primary_key] field",
            ));
        }
        ddl_args.push(quote! {
            ",\n    __last_written_ms INTEGER NOT NULL DEFAULT (unixepoch('now') * 1000)"
        });
    }

    if !pk_field_info.is_empty() {
        let pk_names = pk_field_info
            .iter()
            .map(|(id, _)| id.to_string())
            .collect::<Vec<_>>();
        let pk_str = format!(",\n    PRIMARY KEY ({})", pk_names.join(", "));
        ddl_args.push(quote! { #pk_str });
    }

    ddl_args.push(quote! { "\n);\n" });

    // Append CREATE INDEX statements.
    for idx in &indices {
        let cols: Vec<String> = idx.columns.iter().map(|c| c.to_string()).collect();
        let idx_name = format!("idx_{}_{}", table, cols.join("_"));
        let col_list = cols.join(", ");
        let unique_kw = if idx.unique { "UNIQUE " } else { "" };
        let idx_ddl =
            format!("CREATE {unique_kw}INDEX IF NOT EXISTS {idx_name} ON {table} ({col_list});\n");
        ddl_args.push(quote! { #idx_ddl });
    }

    // SELECT pieces
    let mut sel_args: Vec<TokenStream> = vec![quote! { "SELECT " }];
    for (i, f) in fields.iter().enumerate() {
        if i > 0 {
            sel_args.push(quote! { ", " });
        }
        match &f.kind {
            FieldKind::Column { .. } => {
                let name = f.ident.to_string();
                sel_args.push(quote! { #name });
            }
            FieldKind::ColumnGroup { ty, prefix } => {
                sel_args.push(quote! {
                    ::balsaq::__cf::str_replace!(<#ty as ::balsaq::ColumnGroup>::COLS, "{P}", #prefix)
                });
            }
            FieldKind::ColumnGroupVia { raw_ty, prefix, .. } => {
                sel_args.push(quote! {
                    ::balsaq::__cf::str_replace!(<#raw_ty as ::balsaq::ColumnGroup>::COLS, "{P}", #prefix)
                });
            }
        }
    }
    let from_suffix = format!(" FROM {table}");
    sel_args.push(quote! { #from_suffix });

    // INSERT col and val pieces
    let ins_prefix = format!("INSERT INTO {table} (");
    let mut ins_col_args: Vec<TokenStream> = Vec::new();
    let mut ins_val_args: Vec<TokenStream> = Vec::new();
    for (i, f) in fields.iter().enumerate() {
        if i > 0 {
            ins_col_args.push(quote! { ", " });
            ins_val_args.push(quote! { ", " });
        }
        match &f.kind {
            FieldKind::Column { .. } => {
                let name = f.ident.to_string();
                let param = format!(":{}", f.ident);
                ins_col_args.push(quote! { #name });
                ins_val_args.push(quote! { #param });
            }
            FieldKind::ColumnGroup { ty, prefix } => {
                ins_col_args.push(quote! {
                    ::balsaq::__cf::str_replace!(<#ty as ::balsaq::ColumnGroup>::COLS, "{P}", #prefix)
                });
                ins_val_args.push(quote! {
                    ::balsaq::__cf::str_replace!(<#ty as ::balsaq::ColumnGroup>::VALS, "{P}", #prefix)
                });
            }
            FieldKind::ColumnGroupVia { raw_ty, prefix, .. } => {
                ins_col_args.push(quote! {
                    ::balsaq::__cf::str_replace!(<#raw_ty as ::balsaq::ColumnGroup>::COLS, "{P}", #prefix)
                });
                ins_val_args.push(quote! {
                    ::balsaq::__cf::str_replace!(<#raw_ty as ::balsaq::ColumnGroup>::VALS, "{P}", #prefix)
                });
            }
        }
    }

    let pk_cols = pk_field_info
        .iter()
        .map(|(id, _)| id.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let ins_suffix = if track_last_update {
        format!(
            ") ON CONFLICT({pk_cols}) DO UPDATE SET \
             __last_written_ms = MAX(__last_written_ms, unixepoch('now') * 1000)"
        )
    } else {
        ") ON CONFLICT DO NOTHING".to_owned()
    };

    // from_row and write_params code
    let from_row_fields = fields.iter().map(|f| {
        let ident = &f.ident;
        match &f.kind {
            FieldKind::Column { .. } => {
                let name = f.ident.to_string();
                quote! { #ident: row.get(#name)? }
            }
            FieldKind::ColumnGroup { ty, prefix } => {
                quote! { #ident: <#ty as ::balsaq::ColumnGroup>::cg_read(row, #prefix)? }
            }
            FieldKind::ColumnGroupVia {
                raw_ty,
                field_ty,
                prefix,
            } => {
                quote! {
                    #ident: {
                        let __raw = <#raw_ty as ::balsaq::ColumnGroup>::cg_read(row, #prefix)?;
                        <#field_ty as ::std::convert::TryFrom<#raw_ty>>::try_from(__raw)?
                    }
                }
            }
        }
    });

    let raw_decls: Vec<TokenStream> = fields
        .iter()
        .filter_map(|f| {
            let ident = &f.ident;
            if let FieldKind::ColumnGroupVia {
                raw_ty, field_ty, ..
            } = &f.kind
            {
                let raw_var = quote::format_ident!("__raw_{}", ident);
                Some(quote! {
                    let #raw_var = <#raw_ty as ::std::convert::From<#field_ty>>::from(
                        self.#ident
                    );
                })
            } else {
                None
            }
        })
        .collect();

    let write_stmts = fields.iter().map(|f| {
        let ident = &f.ident;
        match &f.kind {
            FieldKind::Column { .. } => {
                let param = format!(":{}", f.ident);
                quote! {
                    params.push((#param.to_owned(), &self.#ident as &dyn ::rusqlite::types::ToSql));
                }
            }
            FieldKind::ColumnGroup { ty, prefix } => {
                quote! {
                    <#ty as ::balsaq::ColumnGroup>::cg_write(&self.#ident, #prefix, &mut params);
                }
            }
            FieldKind::ColumnGroupVia { raw_ty, prefix, .. } => {
                let raw_var = quote::format_ident!("__raw_{}", ident);
                quote! {
                    <#raw_ty as ::balsaq::ColumnGroup>::cg_write(&#raw_var, #prefix, &mut params);
                }
            }
        }
    });

    // PrimaryKey type and get() method
    let pk_type = match pk_field_info.as_slice() {
        [] => quote! { &'pk ::std::convert::Infallible },
        [(_, ty)] => quote! { &'pk #ty },
        slice => {
            let tys = slice.iter().map(|(_, ty)| ty);
            quote! { (#(&'pk #tys),*) }
        }
    };

    let (get_conn_param, get_body) = match pk_field_info.as_slice() {
        [] => (quote! { _conn }, quote! { match *pk {} }),
        [(col, _)] => {
            let where_clause = format!(" WHERE {} = ?1", col);
            (
                quote! { conn },
                quote! {
                    const SQL: &str =
                        ::balsaq::__cf::concatcp!(#struct_name::SELECT, #where_clause);
                    conn.prepare_cached(SQL)?.query_row((pk,), Self::from_row)
                },
            )
        }
        slice => {
            let where_clause = format!(
                " WHERE {}",
                slice
                    .iter()
                    .enumerate()
                    .map(|(i, (col, _))| format!("{} = ?{}", col, i + 1))
                    .collect::<Vec<_>>()
                    .join(" AND ")
            );
            let indices = (0..slice.len()).map(syn::Index::from);
            (
                quote! { conn },
                quote! {
                    const SQL: &str =
                        ::balsaq::__cf::concatcp!(#struct_name::SELECT, #where_clause);
                    conn.prepare_cached(SQL)?.query_row((#(pk.#indices,)*), Self::from_row)
                },
            )
        }
    };

    // Assemble final output
    let expanded = quote! {
        #item_struct

        impl ::balsaq::Model for #struct_name {
            const CREATE_TABLE: &'static str = ::balsaq::__cf::concatcp!(#(#ddl_args),*);
            const SELECT: &'static str       = ::balsaq::__cf::concatcp!(#(#sel_args),*);
            const INSERT: &'static str       = ::balsaq::__cf::concatcp!(
                #ins_prefix,
                #(#ins_col_args),*,
                ") VALUES (",
                #(#ins_val_args),*,
                #ins_suffix
            );

            type PrimaryKey<'pk> = #pk_type;

            fn from_row(row: &::rusqlite::Row<'_>) -> ::rusqlite::Result<Self> {
                Ok(Self { #(#from_row_fields,)* })
            }

            fn write_params(
                self,
                stmt: &mut ::rusqlite::CachedStatement<'_>,
            ) -> ::rusqlite::Result<usize> {
                #(#raw_decls)*
                let mut params: ::std::vec::Vec<(
                    ::std::string::String,
                    &dyn ::rusqlite::types::ToSql,
                )> = ::std::vec::Vec::new();
                #(#write_stmts)*
                let params_ref: ::std::vec::Vec<(&str, &dyn ::rusqlite::types::ToSql)> =
                    params.iter().map(|(k, v)| (k.as_str(), *v)).collect();
                stmt.execute(params_ref.as_slice())
            }

            fn get<'pk>(
                #get_conn_param: &::rusqlite::Connection,
                pk: Self::PrimaryKey<'pk>,
            ) -> ::rusqlite::Result<Self> {
                #get_body
            }
        }
    };
    Ok(expanded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proc_macro2::TokenStream;
    use quote::quote;

    fn run(attr: TokenStream, item: TokenStream) -> String {
        expand(attr, item).unwrap().to_string()
    }

    fn run_err(attr: TokenStream, item: TokenStream) -> String {
        expand(attr, item).unwrap_err().to_string()
    }

    #[test]
    fn single_column_no_pk_uses_do_nothing() {
        let out = run(
            quote! { "things" },
            quote! {
                pub struct Thing {
                    pub name: String,
                }
            },
        );
        assert!(out.contains("CREATE TABLE IF NOT EXISTS things"));
        assert!(out.contains("name"));
        assert!(out.contains("Column") && out.contains("SQL_TYPE"));
        assert!(!out.contains("PRIMARY KEY"));
        assert!(out.contains("FROM things"));
        assert!(out.contains("INSERT INTO things"));
        assert!(out.contains("ON CONFLICT DO NOTHING"));
        assert!(out.contains(":name"));
    }

    #[test]
    fn primary_key_annotation() {
        let out = run(
            quote! { "commit_root_trees" },
            quote! {
                pub struct CommitRootTree {
                    #[primary_key]
                    pub commit_id: CommitId,
                    #[primary_key]
                    pub position: i64,
                    pub label: String,
                }
            },
        );
        assert!(out.contains("PRIMARY KEY (commit_id, position)"));
        assert!(out.contains("commit_root_trees"));
        assert!(out.contains("commit_id"));
        assert!(out.contains("position"));
        assert!(out.contains("label"));
    }

    #[test]
    fn group_field() {
        let out = run(
            quote! { "posts" },
            quote! {
                pub struct Post {
                    #[primary_key]
                    pub id: BlobId,
                    #[group]
                    pub author: AuthorInfo,
                }
            },
        );
        assert!(out.contains("AuthorInfo"));
        assert!(out.contains("author_"));
        assert!(out.contains("str_replace"));
        assert!(out.contains("cg_read"));
        assert!(out.contains("cg_write"));
    }

    #[test]
    fn group_optional_field() {
        let out = run(
            quote! { "posts" },
            quote! {
                pub struct Post {
                    #[primary_key]
                    pub id: BlobId,
                    #[group]
                    pub sig: Option<SigInfo>,
                }
            },
        );
        assert!(out.contains("SigInfo"));
        assert!(out.contains("sig_"));
        assert!(out.contains("cg_read"));
        assert!(out.contains("cg_write"));
        assert!(out.contains("NULLABLE"));
    }

    #[test]
    fn via_field_uses_try_from_and_from() {
        let out = run(
            quote! { "tree_entries" },
            quote! {
                pub struct TreeEntry {
                    pub tree_id: TreeId,
                    #[group(via = TreeValueRaw)]
                    pub value: TreeValue,
                }
            },
        );
        assert!(out.contains("TreeValueRaw"));
        assert!(out.contains("ColumnGroup"));
        assert!(out.contains("str_replace"));
        assert!(out.contains("value_"));
        assert!(out.contains("cg_read"));
        assert!(out.contains("TryFrom"));
        assert!(out.contains("try_from"));
        assert!(out.contains("__raw_value"));
        assert!(out.contains("From"));
        assert!(out.contains("cg_write"));
        assert!(out.contains("TreeValue"));
    }

    #[test]
    fn error_via_extra_args_rejected() {
        let err = run_err(
            quote! { "things" },
            quote! {
                pub struct Thing {
                    #[primary_key]
                    pub id: i64,
                    #[group(via = ThingRaw, extra)]
                    pub value: ThingDomain,
                }
            },
        );
        assert!(err.contains("no further arguments"));
    }

    #[test]
    fn error_group_and_primary_key_on_same_field() {
        let err = run_err(
            quote! { "posts" },
            quote! {
                pub struct Post {
                    #[group]
                    #[primary_key]
                    pub author: AuthorInfo,
                }
            },
        );
        assert!(err.contains("cannot be both #[group] and #[primary_key]"));
    }

    #[test]
    fn error_group_unknown_arg() {
        let err = run_err(
            quote! { "posts" },
            quote! {
                pub struct Post {
                    #[group(optional)]
                    pub sig: SigInfo,
                }
            },
        );
        assert!(err.contains("expected `via = Type`"));
    }

    #[test]
    fn error_missing_table_arg() {
        let err = run_err(
            quote! {},
            quote! {
                pub struct Foo {
                    pub x: String,
                }
            },
        );
        assert!(!err.is_empty());
    }

    #[test]
    fn error_tuple_struct_rejected() {
        let err = run_err(
            quote! { "things" },
            quote! {
                pub struct Thing(String);
            },
        );
        assert!(err.contains("named fields"));
    }

    #[test]
    fn single_index_generated() {
        let out = run(
            quote! { "widgets" },
            quote! {
                #[index(name)]
                pub struct Widget {
                    #[primary_key]
                    pub id: BlobId,
                    pub name: String,
                }
            },
        );
        assert!(out.contains("CREATE INDEX IF NOT EXISTS idx_widgets_name ON widgets (name)"));
        assert!(!out.contains("# [index]"));
    }

    #[test]
    fn unique_index_generated() {
        let out = run(
            quote! { "users" },
            quote! {
                #[index(email, unique = true)]
                pub struct User {
                    #[primary_key]
                    pub id: i64,
                    pub email: String,
                }
            },
        );
        assert!(out.contains("CREATE UNIQUE INDEX IF NOT EXISTS idx_users_email ON users (email)"));
    }

    #[test]
    fn multi_column_index_generated() {
        let out = run(
            quote! { "posts" },
            quote! {
                #[index(author_id, created_at)]
                pub struct Post {
                    #[primary_key]
                    pub id: i64,
                    pub author_id: i64,
                    pub created_at: i64,
                }
            },
        );
        assert!(out.contains(
            "CREATE INDEX IF NOT EXISTS idx_posts_author_id_created_at ON posts (author_id, created_at)"
        ));
    }

    #[test]
    fn single_pk_generates_primary_key_type_and_get() {
        let out = run(
            quote! { "widgets" },
            quote! {
                pub struct Widget {
                    #[primary_key]
                    pub id: BlobId,
                    pub name: String,
                }
            },
        );
        assert!(out.contains("PrimaryKey"));
        assert!(out.contains("BlobId"));
        assert!(out.contains("WHERE id = ?1"));
        assert!(out.contains("query_row"));
    }

    #[test]
    fn compound_pk_generates_tuple_type_and_get() {
        let out = run(
            quote! { "things" },
            quote! {
                pub struct Thing {
                    #[primary_key]
                    pub a: A,
                    #[primary_key]
                    pub b: i64,
                    pub label: String,
                }
            },
        );
        assert!(out.contains("PrimaryKey"));
        assert!(out.contains("'pk"));
        assert!(out.contains("A") && out.contains("i64"));
        assert!(out.contains("WHERE a = ?1 AND b = ?2"));
        assert!(out.contains("pk . 0") || out.contains("pk.0"));
        assert!(out.contains("pk . 1") || out.contains("pk.1"));
    }

    #[test]
    fn no_pk_generates_infallible() {
        let out = run(
            quote! { "things" },
            quote! {
                pub struct Thing {
                    pub name: String,
                }
            },
        );
        assert!(out.contains("Infallible"));
        assert!(out.contains("match * pk") || out.contains("match *pk"));
        assert!(!out.contains("WHERE"));
    }

    #[test]
    fn error_index_unknown_column() {
        let err = run_err(
            quote! { "things" },
            quote! {
                #[index(nonexistent)]
                pub struct Thing {
                    pub name: String,
                }
            },
        );
        assert!(err.contains("unknown column `nonexistent`"));
    }

    #[test]
    fn track_last_update_ddl_and_insert() {
        let out = run(
            quote! { "commits" },
            quote! {
                #[track_last_update]
                pub struct Commit {
                    #[primary_key]
                    pub id: BlobId,
                    pub data: String,
                }
            },
        );
        // DDL must include the hidden column with its DEFAULT.
        assert!(
            out.contains("__last_written_ms INTEGER NOT NULL DEFAULT (unixepoch('now') * 1000)")
        );
        // INSERT must be a plain INSERT (not INSERT OR IGNORE) …
        assert!(out.contains("INSERT INTO commits"));
        assert!(!out.contains("INSERT OR IGNORE"));
        // … with an ON CONFLICT upsert that applies MAX.
        assert!(out.contains("ON CONFLICT(id) DO UPDATE SET __last_written_ms = MAX(__last_written_ms, unixepoch('now') * 1000)"));
        // SELECT must NOT include __last_written_ms.
        assert!(
            !out.contains("SELECT __last_written_ms") && !out.contains("__last_written_ms FROM")
        );
    }

    #[test]
    fn track_last_update_compound_pk() {
        let out = run(
            quote! { "operation_parents" },
            quote! {
                #[track_last_update]
                pub struct OperationParent {
                    #[primary_key]
                    pub operation_id: OpId,
                    #[primary_key]
                    pub position: i64,
                    pub parent_id: OpId,
                }
            },
        );
        assert!(
            out.contains("ON CONFLICT(operation_id, position) DO UPDATE SET __last_written_ms")
        );
    }

    #[test]
    fn error_track_last_update_without_pk() {
        let err = run_err(
            quote! { "things" },
            quote! {
                #[track_last_update]
                pub struct Thing {
                    pub name: String,
                }
            },
        );
        assert!(err.contains("#[track_last_update] requires at least one #[primary_key]"));
    }

    #[test]
    fn error_track_last_update_with_args() {
        let err = run_err(
            quote! { "things" },
            quote! {
                #[track_last_update(oops)]
                pub struct Thing {
                    #[primary_key]
                    pub id: i64,
                }
            },
        );
        assert!(err.contains("takes no arguments"));
    }

    #[test]
    fn error_index_group_field() {
        let err = run_err(
            quote! { "posts" },
            quote! {
                #[index(author)]
                pub struct Post {
                    #[primary_key]
                    pub id: BlobId,
                    #[group]
                    pub author: AuthorInfo,
                }
            },
        );
        assert!(err.contains("#[group] field"));
    }
}
