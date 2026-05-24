use proc_macro2::TokenStream;
use quote::quote;
use syn::{Error, Item, Result};

pub(crate) fn expand(attr: TokenStream, item: TokenStream) -> Result<TokenStream> {
    if !attr.is_empty() {
        return Err(Error::new_spanned(attr, "#[schema] takes no arguments"));
    }

    let mut module: syn::ItemMod = syn::parse2(item)?;

    let Some((_, ref mut items)) = module.content else {
        return Err(Error::new_spanned(
            &module.ident,
            "#[schema] requires an inline module; `mod name;` form is not supported",
        ));
    };

    let table_structs: Vec<syn::Ident> = items
        .iter()
        .filter_map(|item| {
            if let Item::Struct(s) = item {
                s.attrs
                    .iter()
                    .any(|a| {
                        a.path()
                            .segments
                            .last()
                            .is_some_and(|seg| seg.ident == "table")
                    })
                    .then(|| s.ident.clone())
            } else {
                None
            }
        })
        .collect();

    if table_structs.is_empty() {
        return Err(Error::new_spanned(
            &module.ident,
            "#[schema] module contains no #[table] structs",
        ));
    }

    let schema_item: Item = syn::parse2(quote! {
        pub const SCHEMA: &'static str = ::balsaq::__cf::concatcp!(
            #(<#table_structs as ::balsaq::Model>::CREATE_TABLE),*
        );
    })?;
    items.push(schema_item);

    Ok(quote! { #module })
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    fn run(attr: TokenStream, item: TokenStream) -> String {
        expand(attr, item).unwrap().to_string()
    }

    fn run_err(attr: TokenStream, item: TokenStream) -> String {
        expand(attr, item).unwrap_err().to_string()
    }

    #[test]
    fn single_table_generates_schema() {
        let out = run(
            quote! {},
            quote! {
                mod db {
                    #[table("widgets")]
                    pub struct Widget {}
                }
            },
        );
        assert!(out.contains("SCHEMA"));
        assert!(out.contains("Widget") && out.contains("CREATE_TABLE"));
        assert!(out.contains("concatcp"));
    }

    #[test]
    fn multiple_tables_all_appear_in_schema() {
        let out = run(
            quote! {},
            quote! {
                mod db {
                    #[table("widgets")]
                    pub struct Widget {}
                    #[table("posts")]
                    pub struct Post {}
                }
            },
        );
        assert!(out.contains("Widget") && out.contains("CREATE_TABLE"));
        assert!(out.contains("Post") && out.contains("CREATE_TABLE"));
    }

    #[test]
    fn non_table_items_are_passed_through() {
        let out = run(
            quote! {},
            quote! {
                mod db {
                    pub struct Helper;
                    #[column_set]
                    pub struct Info {}
                    #[table("t")]
                    pub struct T {}
                }
            },
        );
        assert!(out.contains("Helper"));
        assert!(out.contains("Info"));
        // Only T ends up in the SCHEMA Model reference, not Helper or Info.
        assert!(out.contains("CREATE_TABLE"));
        assert!(!out.contains("Helper as"));
    }

    #[test]
    fn error_no_table_structs() {
        let err = run_err(
            quote! {},
            quote! {
                mod db {
                    pub struct NotATable;
                }
            },
        );
        assert!(err.contains("no #[table] structs"));
    }

    #[test]
    fn error_non_inline_module() {
        let err = run_err(quote! {}, quote! { mod db; });
        assert!(err.contains("inline module"));
    }

    #[test]
    fn error_attr_args_rejected() {
        let err = run_err(quote! { something }, quote! { mod db {} });
        assert!(err.contains("takes no arguments"));
    }
}
