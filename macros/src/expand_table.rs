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
    auto_primary_key: bool,
    track_last_update: bool,
}

impl Parse for TableArgs {
    fn parse(input: ParseStream) -> Result<Self> {
        let table_name: LitStr = input.parse()?;
        let mut auto_primary_key = false;
        let mut track_last_update = false;

        while input.peek(Token![,]) {
            let _: Token![,] = input.parse()?;
            if input.is_empty() {
                break; // tolerate trailing comma
            }
            let flag: syn::Ident = input.parse()?;
            if flag == "auto_primary_key" {
                auto_primary_key = true;
            } else if flag == "track_last_update" {
                track_last_update = true;
            } else {
                return Err(Error::new(
                    flag.span(),
                    format!(
                        "unknown `#[table]` flag `{flag}`; expected `auto_primary_key` or `track_last_update`"
                    ),
                ));
            }
        }

        Ok(Self {
            table_name,
            auto_primary_key,
            track_last_update,
        })
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
    /// An unannotated, `#[primary_key]`, or `#[unique]` field — type implements `Column`.
    Column {
        primary_key: bool,
        unique: bool,
        ty: syn::Type,
    },
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
    let TableArgs {
        table_name,
        auto_primary_key,
        track_last_update,
    } = syn::parse2::<TableArgs>(attr)?;
    let table = table_name.value();

    let mut item_struct: ItemStruct = syn::parse2(item)?;
    let struct_name = item_struct.ident.clone();
    let vis = item_struct.vis.clone();

    // Collect and strip #[index] attrs from the struct before re-emitting it.
    let mut indices: Vec<IndexArgs> = Vec::new();
    let mut kept_attrs = Vec::new();
    for attr in item_struct.attrs.drain(..) {
        if attr.path().is_ident("index") {
            indices.push(attr.parse_args::<IndexArgs>()?);
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
        let unique_pos = field.attrs.iter().position(|a| a.path().is_ident("unique"));

        let kind = if let Some(group_pos) = group_pos {
            if let Some(pk_pos) = pk_pos {
                return Err(Error::new(
                    field.attrs[pk_pos].span(),
                    "a field cannot be both #[group] and #[primary_key]",
                ));
            }
            if let Some(unique_pos) = unique_pos {
                return Err(Error::new(
                    field.attrs[unique_pos].span(),
                    "a field cannot be both #[group] and #[unique]",
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
            if let Some(unique_pos) = unique_pos {
                return Err(Error::new(
                    field.attrs[unique_pos].span(),
                    "a field cannot be both #[primary_key] and #[unique]",
                ));
            }
            field.attrs.remove(pk_pos);
            FieldKind::Column {
                primary_key: true,
                unique: false,
                ty: field.ty.clone(),
            }
        } else if let Some(unique_pos) = unique_pos {
            field.attrs.remove(unique_pos);
            FieldKind::Column {
                primary_key: false,
                unique: true,
                ty: field.ty.clone(),
            }
        } else {
            FieldKind::Column {
                primary_key: false,
                unique: false,
                ty: field.ty.clone(),
            }
        };

        fields.push(FieldInfo { ident, kind });
    }

    // Collect PK and unique fields.
    let pk_field_info: Vec<(&syn::Ident, &syn::Type)> = fields
        .iter()
        .filter_map(|f| {
            if let FieldKind::Column {
                primary_key: true,
                ty,
                ..
            } = &f.kind
            {
                Some((&f.ident, ty))
            } else {
                None
            }
        })
        .collect();

    let unique_field_info: Vec<(&syn::Ident, &syn::Type)> = fields
        .iter()
        .filter_map(|f| {
            if let FieldKind::Column {
                unique: true, ty, ..
            } = &f.kind
            {
                Some((&f.ident, ty))
            } else {
                None
            }
        })
        .collect();

    // Validate constraint rules.
    if auto_primary_key && !pk_field_info.is_empty() {
        return Err(Error::new(
            struct_name.span(),
            "`auto_primary_key` and `#[primary_key]` cannot both be used on the same table",
        ));
    }
    if !unique_field_info.is_empty() && !auto_primary_key {
        return Err(Error::new(
            unique_field_info[0].0.span(),
            "`#[unique]` requires `auto_primary_key` on the table",
        ));
    }
    if track_last_update {
        let valid =
            !pk_field_info.is_empty() || (auto_primary_key && !unique_field_info.is_empty());
        if !valid {
            return Err(Error::new(
                struct_name.span(),
                "`track_last_update` requires either a `#[primary_key]` field \
                 or `auto_primary_key` with at least one `#[unique]` field",
            ));
        }
    }

    // Validate #[index] column names.
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

    // CREATE TABLE DDL pieces.
    let mut ddl_args: Vec<TokenStream> = Vec::new();
    let create_prefix = format!("CREATE TABLE IF NOT EXISTS {table} (\n");
    ddl_args.push(quote! { #create_prefix });

    // For auto_primary_key, emit row_id as the first column.
    if auto_primary_key {
        ddl_args.push(quote! { "    row_id INTEGER PRIMARY KEY" });
    }

    for (i, f) in fields.iter().enumerate() {
        // Comma separator: always needed after row_id (if auto_primary_key), or between fields.
        if i > 0 || auto_primary_key {
            ddl_args.push(quote! { ",\n" });
        }
        match &f.kind {
            FieldKind::Column { ty, unique, .. } => {
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
                if *unique {
                    ddl_args.push(quote! { " UNIQUE" });
                }
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

    if track_last_update {
        ddl_args.push(quote! {
            ",\n    __last_written_ms INTEGER NOT NULL DEFAULT (unixepoch('now') * 1000)"
        });
    }

    // For #[primary_key] tables, add the PRIMARY KEY constraint and use WITHOUT ROWID.
    let without_rowid = !pk_field_info.is_empty();
    if without_rowid {
        let pk_names = pk_field_info
            .iter()
            .map(|(id, _)| id.to_string())
            .collect::<Vec<_>>();
        let pk_str = format!(",\n    PRIMARY KEY ({})", pk_names.join(", "));
        ddl_args.push(quote! { #pk_str });
    }

    let close = if without_rowid {
        "\n) WITHOUT ROWID;\n"
    } else {
        "\n);\n"
    };
    ddl_args.push(quote! { #close });

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

    // SELECT column list (reused in SELECT constant and get_by_xyz queries).
    let mut sel_col_args: Vec<TokenStream> = Vec::new();
    for (i, f) in fields.iter().enumerate() {
        if i > 0 {
            sel_col_args.push(quote! { ", " });
        }
        match &f.kind {
            FieldKind::Column { .. } => {
                let name = f.ident.to_string();
                sel_col_args.push(quote! { #name });
            }
            FieldKind::ColumnGroup { ty, prefix } => {
                sel_col_args.push(quote! {
                    ::balsaq::__cf::str_replace!(<#ty as ::balsaq::ColumnGroup>::COLS, "{P}", #prefix)
                });
            }
            FieldKind::ColumnGroupVia { raw_ty, prefix, .. } => {
                sel_col_args.push(quote! {
                    ::balsaq::__cf::str_replace!(<#raw_ty as ::balsaq::ColumnGroup>::COLS, "{P}", #prefix)
                });
            }
        }
    }

    // SELECT constant: SELECT <cols> FROM <table>.
    let from_suffix = format!(" FROM {table}");
    let sel_args: Vec<TokenStream> = std::iter::once(quote! { "SELECT " })
        .chain(sel_col_args.iter().cloned())
        .chain(std::iter::once(quote! { #from_suffix }))
        .collect();

    // INSERT col and val pieces.
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

    let ins_suffix = if auto_primary_key {
        // Always use ON CONFLICT ... DO UPDATE so that RETURNING row_id fires even on conflict.
        let conflict_target = if !unique_field_info.is_empty() {
            let cols = unique_field_info
                .iter()
                .map(|(id, _)| id.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            format!("({cols})")
        } else {
            String::new() // no unique column → no conflict possible, DO NOTHING is fine
        };
        let update_clause = if track_last_update {
            "__last_written_ms = MAX(__last_written_ms, unixepoch('now') * 1000)".to_owned()
        } else if conflict_target.is_empty() {
            // No unique column and no tracking: plain insert, no conflict handling needed.
            // We still need RETURNING, so use DO NOTHING (conflict can't happen anyway).
            return Err(Error::new(
                struct_name.span(),
                "unreachable: auto_primary_key without unique fields should not reach this path",
            ));
        } else {
            // No-op update so RETURNING fires on both insert and conflict paths.
            "row_id = row_id".to_owned()
        };

        if conflict_target.is_empty() {
            // auto_primary_key, no unique fields: plain insert, no conflict possible.
            ") ON CONFLICT DO NOTHING RETURNING row_id".to_owned()
        } else {
            format!(") ON CONFLICT{conflict_target} DO UPDATE SET {update_clause} RETURNING row_id")
        }
    } else {
        let pk_cols = pk_field_info
            .iter()
            .map(|(id, _)| id.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        if track_last_update {
            format!(
                ") ON CONFLICT({pk_cols}) DO UPDATE SET \
                 __last_written_ms = MAX(__last_written_ms, unixepoch('now') * 1000)"
            )
        } else {
            ") ON CONFLICT DO NOTHING".to_owned()
        }
    };

    // from_row and write_stmts code (shared between do_insert implementations).
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

    // PrimaryKey type and get() method.
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

    // InsertId type and do_insert body.
    let row_id_name = quote::format_ident!("{}RowId", struct_name);
    let (insert_id_type, do_insert_body) = if auto_primary_key {
        let body = quote! {
            #(#raw_decls)*
            let mut params: ::std::vec::Vec<(
                ::std::string::String,
                &dyn ::rusqlite::types::ToSql,
            )> = ::std::vec::Vec::new();
            #(#write_stmts)*
            let params_ref: ::std::vec::Vec<(&str, &dyn ::rusqlite::types::ToSql)> =
                params.iter().map(|(k, v)| (k.as_str(), *v)).collect();
            let __row_id: i64 = conn
                .prepare_cached(Self::INSERT)?
                .query_row(params_ref.as_slice(), |r| r.get(0))?;
            ::std::result::Result::Ok(#row_id_name(__row_id))
        };
        (quote! { #row_id_name }, body)
    } else {
        let body = quote! {
            #(#raw_decls)*
            let mut params: ::std::vec::Vec<(
                ::std::string::String,
                &dyn ::rusqlite::types::ToSql,
            )> = ::std::vec::Vec::new();
            #(#write_stmts)*
            let params_ref: ::std::vec::Vec<(&str, &dyn ::rusqlite::types::ToSql)> =
                params.iter().map(|(k, v)| (k.as_str(), *v)).collect();
            conn.prepare_cached(Self::INSERT)?.execute(params_ref.as_slice())?;
            ::std::result::Result::Ok(())
        };
        (quote! { () }, body)
    };

    // get_by_xyz methods for each #[unique] field (only generated for auto_primary_key tables).
    let get_by_methods: Vec<TokenStream> = unique_field_info
        .iter()
        .map(|(field_ident, field_ty)| {
            let method_name = quote::format_ident!("get_by_{}", field_ident);
            let where_clause = format!(" WHERE {} = ?1", field_ident);
            let table_str = table.as_str();
            quote! {
                #vis fn #method_name(
                    conn: &::rusqlite::Connection,
                    val: &#field_ty,
                ) -> ::rusqlite::Result<(#row_id_name, Self)> {
                    const SQL: &'static str = ::balsaq::__cf::concatcp!(
                        "SELECT ", #(#sel_col_args),*, ", row_id FROM ", #table_str, #where_clause
                    );
                    conn.prepare_cached(SQL)?.query_row((val,), |row| {
                        ::std::result::Result::Ok((
                            #row_id_name(row.get("row_id")?),
                            Self::from_row(row)?,
                        ))
                    })
                }
            }
        })
        .collect();

    // Only emit the RowId newtype and get_by_xyz impl block for auto_primary_key tables.
    let row_id_and_impl = if auto_primary_key {
        let get_by_block = if !get_by_methods.is_empty() {
            quote! {
                impl #struct_name {
                    #(#get_by_methods)*
                }
            }
        } else {
            quote! {}
        };
        quote! {
            #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
            #vis struct #row_id_name(pub i64);

            #get_by_block
        }
    } else {
        quote! {}
    };

    // Assemble final output.
    let expanded = quote! {
        #item_struct

        #row_id_and_impl

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
            type InsertId = #insert_id_type;

            fn from_row(row: &::rusqlite::Row<'_>) -> ::rusqlite::Result<Self> {
                Ok(Self { #(#from_row_fields,)* })
            }

            fn do_insert(
                self,
                conn: &::rusqlite::Connection,
            ) -> ::rusqlite::Result<Self::InsertId> {
                #do_insert_body
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
        let tokens = expand(attr, item).unwrap();
        let file: syn::File = syn::parse2(tokens).unwrap();
        prettyplease::unparse(&file)
    }

    fn run_err(attr: TokenStream, item: TokenStream) -> String {
        expand(attr, item).unwrap_err().to_string()
    }

    #[test]
    fn single_column_no_pk_uses_do_nothing() {
        insta::assert_snapshot!(run(
            quote! { "things" },
            quote! {
                pub struct Thing {
                    pub name: String,
                }
            },
        ));
    }

    #[test]
    fn primary_key_annotation() {
        insta::assert_snapshot!(run(
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
        ));
    }

    #[test]
    fn group_field() {
        insta::assert_snapshot!(run(
            quote! { "posts" },
            quote! {
                pub struct Post {
                    #[primary_key]
                    pub id: BlobId,
                    #[group]
                    pub author: AuthorInfo,
                }
            },
        ));
    }

    #[test]
    fn group_optional_field() {
        insta::assert_snapshot!(run(
            quote! { "posts" },
            quote! {
                pub struct Post {
                    #[primary_key]
                    pub id: BlobId,
                    #[group]
                    pub sig: Option<SigInfo>,
                }
            },
        ));
    }

    #[test]
    fn via_field_uses_try_from_and_from() {
        insta::assert_snapshot!(run(
            quote! { "tree_entries" },
            quote! {
                pub struct TreeEntry {
                    pub tree_id: TreeId,
                    #[group(via = TreeValueRaw)]
                    pub value: TreeValue,
                }
            },
        ));
    }

    #[test]
    fn auto_primary_key_with_unique() {
        insta::assert_snapshot!(run(
            quote! { "commits", auto_primary_key },
            quote! {
                pub struct Commit {
                    #[unique]
                    pub hash: BlobId,
                    pub data: String,
                }
            },
        ));
    }

    #[test]
    fn auto_primary_key_with_unique_and_track() {
        insta::assert_snapshot!(run(
            quote! { "commits", auto_primary_key, track_last_update },
            quote! {
                pub struct Commit {
                    #[unique]
                    pub hash: BlobId,
                    pub data: String,
                }
            },
        ));
    }

    #[test]
    fn auto_primary_key_no_unique() {
        insta::assert_snapshot!(run(
            quote! { "things", auto_primary_key },
            quote! {
                pub struct Thing {
                    pub name: String,
                }
            },
        ));
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
    fn error_group_and_unique_on_same_field() {
        let err = run_err(
            quote! { "posts", auto_primary_key },
            quote! {
                pub struct Post {
                    #[group]
                    #[unique]
                    pub author: AuthorInfo,
                }
            },
        );
        assert!(err.contains("cannot be both #[group] and #[unique]"));
    }

    #[test]
    fn error_primary_key_and_unique_on_same_field() {
        let err = run_err(
            quote! { "things" },
            quote! {
                pub struct Thing {
                    #[primary_key]
                    #[unique]
                    pub id: i64,
                }
            },
        );
        assert!(err.contains("cannot be both #[primary_key] and #[unique]"));
    }

    #[test]
    fn error_auto_primary_key_and_field_primary_key() {
        let err = run_err(
            quote! { "things", auto_primary_key },
            quote! {
                pub struct Thing {
                    #[primary_key]
                    pub id: i64,
                }
            },
        );
        assert!(err.contains("`auto_primary_key` and `#[primary_key]` cannot both be used"));
    }

    #[test]
    fn error_unique_without_auto_primary_key() {
        let err = run_err(
            quote! { "things" },
            quote! {
                pub struct Thing {
                    #[unique]
                    pub id: i64,
                }
            },
        );
        assert!(err.contains("`#[unique]` requires `auto_primary_key`"));
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
        insta::assert_snapshot!(run(
            quote! { "widgets" },
            quote! {
                #[index(name)]
                pub struct Widget {
                    #[primary_key]
                    pub id: BlobId,
                    pub name: String,
                }
            },
        ));
    }

    #[test]
    fn unique_index_generated() {
        insta::assert_snapshot!(run(
            quote! { "users" },
            quote! {
                #[index(email, unique = true)]
                pub struct User {
                    #[primary_key]
                    pub id: i64,
                    pub email: String,
                }
            },
        ));
    }

    #[test]
    fn multi_column_index_generated() {
        insta::assert_snapshot!(run(
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
        ));
    }

    #[test]
    fn single_pk_generates_primary_key_type_and_get() {
        insta::assert_snapshot!(run(
            quote! { "widgets" },
            quote! {
                pub struct Widget {
                    #[primary_key]
                    pub id: BlobId,
                    pub name: String,
                }
            },
        ));
    }

    #[test]
    fn compound_pk_generates_tuple_type_and_get() {
        insta::assert_snapshot!(run(
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
        ));
    }

    #[test]
    fn no_pk_generates_infallible() {
        insta::assert_snapshot!(run(
            quote! { "things" },
            quote! {
                pub struct Thing {
                    pub name: String,
                }
            },
        ));
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
        insta::assert_snapshot!(run(
            quote! { "commits", track_last_update },
            quote! {
                pub struct Commit {
                    #[primary_key]
                    pub id: BlobId,
                    pub data: String,
                }
            },
        ));
    }

    #[test]
    fn track_last_update_compound_pk() {
        insta::assert_snapshot!(run(
            quote! { "operation_parents", track_last_update },
            quote! {
                pub struct OperationParent {
                    #[primary_key]
                    pub operation_id: OpId,
                    #[primary_key]
                    pub position: i64,
                    pub parent_id: OpId,
                }
            },
        ));
    }

    #[test]
    fn error_track_last_update_without_pk() {
        let err = run_err(
            quote! { "things", track_last_update },
            quote! {
                pub struct Thing {
                    pub name: String,
                }
            },
        );
        assert!(err.contains("`track_last_update` requires"));
    }

    #[test]
    fn error_unknown_table_flag() {
        let err = run_err(
            quote! { "things", oops },
            quote! {
                pub struct Thing {
                    pub name: String,
                }
            },
        );
        assert!(err.contains("unknown `#[table]` flag `oops`"));
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
