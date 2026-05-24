use proc_macro2::TokenStream;
use quote::quote;
use syn::{Error, Fields, ItemStruct, Result};

struct ColInfo {
    field_name: syn::Ident,
    field_ty: syn::Type,
}

pub(crate) fn expand(item: TokenStream) -> Result<TokenStream> {
    let item_struct: ItemStruct = syn::parse2(item)?;
    let struct_name = item_struct.ident.clone();

    let Fields::Named(named) = &item_struct.fields else {
        return Err(Error::new(
            struct_name.span(),
            "#[group] only supports structs with named fields",
        ));
    };

    let cols: Vec<ColInfo> = named
        .named
        .iter()
        .map(|field| ColInfo {
            field_name: field.ident.clone().expect("named field has ident"),
            field_ty: field.ty.clone(),
        })
        .collect();

    if cols.is_empty() {
        return Err(Error::new(
            struct_name.span(),
            "#[group] struct must have at least one field",
        ));
    }

    // Build {P}/{NN}-templated DDL constant
    let mut ddl_args: Vec<TokenStream> = Vec::new();
    for (i, c) in cols.iter().enumerate() {
        if i > 0 {
            ddl_args.push(quote! { ",\n" });
        }
        let ty = &c.field_ty;
        let prefix_str = format!("    {{P}}{} ", c.field_name);
        // The col name in this group is "{P}fieldname"; after the outer macro
        // resolves {P}, the {col} reference in CHECK constraints resolves too.
        let col_in_group = format!("{{P}}{}", c.field_name);
        ddl_args.push(quote! { #prefix_str });
        ddl_args.push(quote! {
            ::balsaq::__cf::str_replace!(
                <#ty as ::balsaq::Column>::SQL_TYPE, "{col}", #col_in_group
            )
        });
        ddl_args.push(quote! { ::balsaq::__nn_placeholder(<#ty as ::balsaq::Column>::NULLABLE) });
    }

    let col_names: Vec<String> = cols
        .iter()
        .map(|c| format!("{{P}}{}", c.field_name))
        .collect();
    let cols_str = col_names.join(", ");
    let vals_str = col_names
        .iter()
        .map(|n| format!(":{n}"))
        .collect::<Vec<_>>()
        .join(", ");

    // cg_read
    let read_fields = cols.iter().map(|c| {
        let ident = &c.field_name;
        let name = c.field_name.to_string();
        quote! { #ident: row.get(format!("{prefix}{}", #name).as_str())? }
    });

    // cg_write
    let write_entries = cols.iter().map(|c| {
        let ident = &c.field_name;
        let name = c.field_name.to_string();
        quote! {
            out.push((format!(":{prefix}{}", #name), &self.#ident as &dyn ::rusqlite::types::ToSql));
        }
    });

    // cg_read_optional (all-null check)
    //
    // Use get_ref to inspect the raw ValueRef for each column. If every column is NULL the group
    // is absent (None). Otherwise the struct is read using the original field types — get is a
    // cheap re-parse of already-buffered data, not a second DB round-trip.
    let all_null_checks = cols.iter().map(|c| {
        let name = c.field_name.to_string();
        quote! {
            matches!(
                row.get_ref(format!("{prefix}{}", #name).as_str())?,
                ::rusqlite::types::ValueRef::Null
            )
        }
    });

    // Re-read using original types once we know the group is present.
    let present_fields = cols.iter().map(|c| {
        let ident = &c.field_name;
        let name = c.field_name.to_string();
        quote! { #ident: row.get(format!("{prefix}{}", #name).as_str())? }
    });

    // cg_write_null
    let null_entries = cols.iter().map(|c| {
        let name = c.field_name.to_string();
        quote! {
            out.push((format!(":{prefix}{}", #name), &NULL as &dyn ::rusqlite::types::ToSql));
        }
    });

    Ok(quote! {
        #item_struct

        impl ::balsaq::ColumnGroup for #struct_name {
            const DDL: &'static str = ::balsaq::__cf::concatcp!(#(#ddl_args),*);
            const COLS: &'static str = #cols_str;
            const VALS: &'static str = #vals_str;
            const NULLABLE: bool = false;

            fn cg_read(row: &::rusqlite::Row<'_>, prefix: &str) -> ::rusqlite::Result<Self> {
                Ok(Self {
                    #(#read_fields,)*
                })
            }

            fn cg_write<'a>(
                &'a self,
                prefix: &str,
                out: &mut ::std::vec::Vec<(::std::string::String, &'a dyn ::rusqlite::types::ToSql)>,
            ) {
                #(#write_entries)*
            }

            fn cg_read_optional(
                row: &::rusqlite::Row<'_>,
                prefix: &str,
            ) -> ::rusqlite::Result<::std::option::Option<Self>> {
                if #(#all_null_checks)&&* {
                    ::std::result::Result::Ok(::std::option::Option::None)
                } else {
                    ::std::result::Result::Ok(::std::option::Option::Some(Self {
                        #(#present_fields,)*
                    }))
                }
            }

            fn cg_write_null<'a>(
                prefix: &str,
                out: &mut ::std::vec::Vec<(::std::string::String, &'a dyn ::rusqlite::types::ToSql)>,
            ) {
                static NULL: ::rusqlite::types::Null = ::rusqlite::types::Null;
                #(#null_entries)*
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    fn run(item: TokenStream) -> String {
        expand(item).unwrap().to_string()
    }

    fn run_err(item: TokenStream) -> String {
        expand(item).unwrap_err().to_string()
    }

    #[test]
    fn generates_placeholder_constants() {
        let out = run(quote! {
            pub struct Point {
                pub x: i64,
                pub y: i64,
            }
        });
        assert!(out.contains("{P}x"));
        assert!(out.contains("{P}y"));
        assert!(out.contains("{P}x, {P}y"));
        assert!(out.contains(":{P}x, :{P}y"));
        assert!(out.contains("Column") && out.contains("SQL_TYPE"));
    }

    #[test]
    fn always_emits_all_four_methods() {
        let out = run(quote! {
            pub struct Sig {
                pub data: Vec<u8>,
                pub hash: Vec<u8>,
            }
        });
        assert!(out.contains("cg_read"));
        assert!(out.contains("cg_write"));
        assert!(out.contains("cg_read_optional"));
        assert!(out.contains("cg_write_null"));
        assert!(out.contains("Option"));
        assert!(out.contains("Null"));
    }

    #[test]
    fn all_columns_checked_for_null() {
        let out = run(quote! {
            pub struct AllOpt {
                pub label: Option<String>,
                pub note: Option<String>,
            }
        });
        // Both columns must appear in the all-null check, not just the first.
        assert!(out.contains("label"));
        assert!(out.contains("note"));
        assert!(out.contains("ValueRef :: Null") || out.contains("ValueRef::Null"));
        assert!(out.contains("matches !") || out.contains("matches!"));
    }

    #[test]
    fn error_tuple_struct_rejected() {
        let err = run_err(quote! { pub struct Bad(i64); });
        assert!(err.contains("named fields"));
    }
}
